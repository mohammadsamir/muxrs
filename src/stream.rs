use anyhow::Result;
use log::trace;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::task::{Context, Poll, Waker};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::{self, AsyncRead, ReadBuf};
use tokio::pin;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;

use crate::frame::Frame;
use crate::header::Flag;
use crate::header::FrameType;
use crate::header::MuxHeader;
use crate::header::HEADER_LENGTH;

pub struct Shared {
    waker: Option<Waker>,
    write_waker: Option<Waker>,
}

impl Shared {
    pub fn new() -> Self {
        Shared {
            waker: None,
            write_waker: None,
        }
    }
}

pub struct Stream {
    id: u32,
    in_buf: Arc<Mutex<Vec<u8>>>,
    writer: Option<Sender<(MuxHeader, Vec<u8>)>>,
    shared: Arc<StdMutex<Shared>>,
    ready: Arc<AtomicBool>,
}

impl Clone for Stream {
    fn clone(&self) -> Self {
        Stream {
            id: self.id,
            in_buf: self.in_buf.clone(),
            writer: None,
            shared: self.shared.clone(),
            ready: self.ready.clone(),
        }
    }
}

impl Stream {
    pub async fn new(id: u32) -> Self {
        let buf = Arc::new(Mutex::new(vec![]));
        let stream = Stream {
            id,
            in_buf: buf.clone(),
            writer: None,
            shared: Arc::new(StdMutex::new(Shared::new())),
            ready: Arc::new(AtomicBool::new(false)),
        };
        stream
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn set_outbound_writer(&mut self, writer: Sender<(MuxHeader, Vec<u8>)>) {
        self.writer = Some(writer)
    }

    pub async fn write_to_buffer(&self, frame: Frame) -> Result<usize> {
        let mut buf = self.in_buf.lock().await;
        if frame.header().len() == 0 && frame.is_syn() {
            drop(buf);
            trace!("SYN FRAME; SKIPPING");
            return Ok(0);
        } else if frame.header().len() == 0 && frame.is_ack() {
            self.ready.store(true, Ordering::Relaxed);
            self.wake_write();
        }
        let (header, bytes) = frame.split();
        let _ = buf
            .write(&header.to_bytes())
            .map_err(|e| anyhow::Error::from(e));
        let res = buf.write(&bytes).map_err(|e| anyhow::Error::from(e));
        self.wake();
        res
    }

    pub(crate) async fn write_frame(&mut self, frame: Frame) -> Result<(), ()> {
        let (header, bytes) = frame.split();
        self.writer
            .as_ref()
            .unwrap()
            .send((header, bytes))
            .await
            .map_err(|_| ())
    }

    pub fn wake(&self) {
        let shared = self.shared.lock().unwrap();
        if shared.waker.is_some() {
            shared.waker.as_ref().unwrap().wake_by_ref();
        }
    }

    pub fn wake_write(&self) {
        let shared = self.shared.lock().unwrap();
        if shared.write_waker.is_some() {
            shared.write_waker.as_ref().unwrap().wake_by_ref();
        }
    }

    pub async fn read_frame(&mut self) -> Result<Vec<u8>> {
        let mut buf = [0u8; HEADER_LENGTH];
        self.read(&mut buf).await?;
        let header = MuxHeader::from_bytes(buf);

        let mut buf = vec![0u8; header.len()];
        self.read(&mut buf).await?;
        Ok(buf)
    }
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.in_buf.try_lock() {
            Ok(mut b) => {
                if b.len() > 0 {
                    if b.len() > buf.remaining() {
                        buf.put_slice(&b.drain(0..buf.remaining()).collect::<Vec<u8>>());
                    } else {
                        buf.put_slice(&b);
                        *b = vec![];
                    }
                    return Poll::Ready(Ok(()));
                }
            }
            _ => {}
        }
        let mut shared = self.shared.lock().unwrap();
        shared.waker = Some(cx.waker().to_owned());
        return Poll::Pending;
    }
}

// only used externally. always assume data frames and inject headers
impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        if !self.ready.load(Ordering::Relaxed) {
            trace!("stream {} is not ready for writes", { self.id() });
            let mut shared = self.shared.lock().unwrap();
            shared.write_waker = Some(cx.waker().to_owned());

            return Poll::Pending;
        }
        let header = MuxHeader::new(
            self.id,
            buf.len() as u32,
            Some(Flag::Syn),
            Some(FrameType::Data),
        );
        let send = self.writer.as_ref().unwrap().send((header, buf.to_vec()));
        pin!(send);
        match send.poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(sent) => Poll::Ready(
                sent.map(|_| buf.len() + HEADER_LENGTH)
                    .map_err(|e| io::Error::other(e)),
            ),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }
}
