use anyhow::{Context, Result};
use futures::future::{poll_fn, Either};
use log::trace;
use log::{debug, error};
use parking_lot::Mutex as SyncMutex;
use parking_lot::RwLock as SyncRwLock;
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll, Waker};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{mpsc, Mutex, RwLock};

use crate::frame::Frame;
use crate::header::{Flag, FrameType};
use crate::header::{MuxHeader, HEADER_LENGTH};
use crate::stream::Stream;

#[derive(Debug)]
pub enum MuxMode {
    Server, //even
    Client, //odd
}

impl MuxMode {
    fn valid(&self, id: u32) -> bool {
        match self {
            Self::Server => Self::is_server(id),
            Self::Client => !Self::is_server(id),
        }
    }

    fn is_server(id: u32) -> bool {
        id % 2 == 0
    }

    fn from_id(id: u32) -> Self {
        if Self::is_server(id) {
            return Self::Server;
        }
        return Self::Client;
    }
}

pub struct MuxSession<T: AsyncRead + AsyncWrite> {
    inner: Arc<Mutex<T>>,
    next_id: AtomicUsize,
    outbound_writer: Sender<(MuxHeader, Vec<u8>)>,
    outbound_reader: Mutex<Receiver<(MuxHeader, Vec<u8>)>>,
    streams: RwLock<HashMap<u32, Stream>>,
    inbound_streams: Arc<SyncMutex<Vec<Stream>>>,
    inbound_waker: SyncRwLock<Option<Waker>>,
}

impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> MuxSession<T> {
    pub async fn new(inner: T) -> Arc<Self> {
        let (outbound_writer, outbound_reader) = mpsc::channel::<(MuxHeader, Vec<u8>)>(100);
        let session = MuxSession {
            inner: Arc::new(Mutex::new(inner)),
            next_id: AtomicUsize::new(1),
            outbound_writer,
            outbound_reader: Mutex::new(outbound_reader),
            streams: RwLock::new(HashMap::new()),
            inbound_streams: Arc::new(SyncMutex::new(Vec::new())),
            inbound_waker: SyncRwLock::new(None),
        };
        let session = Arc::new(session);
        tokio::spawn(session.clone().run());
        session
    }

    pub async fn open(&self, mode: MuxMode) -> Stream {
        let mut stream = self.open_internal(mode, None).await;

        let _ = stream
            .write_frame(Frame {
                header: MuxHeader::new(stream.id(), 0, Some(Flag::Syn), Some(FrameType::Data)),
                data: vec![],
            })
            .await;
        stream
    }

    async fn open_internal(&self, mode: MuxMode, id: Option<u32>) -> Stream {
        let final_id;
        match id {
            Some(id) => final_id = id,
            None => {
                let mut next_id: usize;
                loop {
                    next_id = self.next_id.load(std::sync::atomic::Ordering::Relaxed);

                    if self
                        .next_id
                        .compare_exchange(
                            next_id,
                            next_id + 1,
                            std::sync::atomic::Ordering::Acquire,
                            std::sync::atomic::Ordering::Relaxed,
                        )
                        .is_ok()
                        && mode.valid(next_id as u32)
                    {
                        break;
                    }
                }
                final_id = next_id as u32;
            }
        }
        debug!("Opening a new stream id: {}, mode: {:?}", final_id, mode);
        let stream = Stream::new(final_id).await;
        let mut cloned = stream.clone();
        cloned.set_outbound_writer(self.outbound_writer.clone());
        self.streams.write().await.insert(final_id, stream);

        cloned
    }

    async fn inbound_write(&self, frame: Frame) -> Result<usize> {
        {
            let streams = self.streams.read().await;
            match streams.get(&frame.header().id()) {
                Some(stream) => return stream.write_to_buffer(frame).await,
                None => {}
            }
        }

        let new_stream = self
            .open_internal(
                MuxMode::from_id(frame.header().id()),
                Some(frame.header().id()),
            )
            .await;
        {
            let mut inbound_streams = self.inbound_streams.lock();
            inbound_streams.push(new_stream);
        }

        {
            let waker = self.inbound_waker.read();
            if waker.is_some() {
                waker.as_ref().unwrap().wake_by_ref();
            } else {
            }
        }

        let streams = self.streams.read().await;
        streams
            .get(&frame.header().id())
            .unwrap()
            .write_to_buffer(frame)
            .await
    }

    pub fn poll_inbound_streams(&self, cx: &mut TaskContext<'_>) -> Poll<Stream> {
        {
            let mut inbound_streams = self.inbound_streams.lock();
            if inbound_streams.len() > 0 {
                let stream = inbound_streams.pop().unwrap();
                return Poll::Ready(stream);
            }
        }
        *self.inbound_waker.write() = Some(cx.waker().to_owned());
        Poll::Pending
    }

    pub async fn accept(&self) -> Stream {
        poll_fn(|cx| self.poll_inbound_streams(cx)).await
    }

    async fn poll_channel(&self) -> Result<(MuxHeader, Vec<u8>)> {
        self.outbound_reader
            .lock()
            .await
            .recv()
            .await
            .context("channel closed")
    }

    async fn write_io(&self, bytes: &[u8]) -> Result<usize> {
        trace!("writing io {:?}", bytes);
        let mut socket = self.inner.lock().await;
        socket.write(bytes).await.map_err(|e| anyhow::Error::new(e))
    }

    async fn poll_io(&self) -> Result<Frame> {
        let mut socket = self.inner.lock().await;
        let mut buf = [0u8; HEADER_LENGTH];
        match socket.read_exact(&mut buf).await {
            Ok(_) => {
                let header = MuxHeader::from_bytes(buf);
                let mut buf = vec![0u8; header.len()];
                match socket.read_exact(&mut buf).await {
                    Ok(_) => {
                        let frame = Frame::new(header, buf);
                        Ok(frame)
                    }
                    Err(e) => Err(anyhow::Error::new(e)),
                }
            }
            Err(e) => Err(anyhow::Error::new(e)),
        }
    }

    pub async fn run(self: Arc<Self>) {
        loop {
            match futures::future::select(Box::pin(self.poll_channel()), Box::pin(self.poll_io()))
                .await
            {
                Either::Left((outbound_write, io)) => {
                    drop(io);
                    match outbound_write {
                        Ok((header, frame)) => {
                            let mut bytes = vec![];
                            bytes.extend_from_slice(&header.to_bytes());
                            bytes.extend_from_slice(&frame);
                            let _ = self.write_io(&bytes).await;
                        }
                        Err(err) => error!("{:?}", err),
                    }
                }
                Either::Right((inbound, ch)) => {
                    drop(ch);
                    match inbound {
                        Ok(mut frame) => {
                            trace!("frame received: {:?}", frame);

                            if frame.header().len() == 0 && frame.is_syn() {
                                frame.set_flag(Flag::Ack);
                                let _ = self.write_io(&frame.as_bytes()).await;
                            } else {
                                let _ = self.inbound_write(frame).await;
                            }
                        }
                        Err(err) => {
                            error!("read failed {:?}", err);
                            break;
                        }
                    }
                }
            }
        }
    }
}
