use anyhow::Result;
use futures::channel::mpsc::Sender;
use futures::{AsyncRead, AsyncReadExt, AsyncWrite};
use log::trace;
use parking_lot::Mutex;
use std::io::Write;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

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
    shared: Arc<Mutex<Shared>>,
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
            shared: Arc::new(Mutex::new(Shared::new())),
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
        let mut buf = self.in_buf.lock();
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

    pub(crate) async fn write_frame(&mut self, frame: Frame) -> Result<()> {
        let (header, bytes) = frame.split();
        self.writer
            .as_mut()
            .unwrap()
            .try_send((header, bytes))
            .map_err(|e| anyhow::Error::new(e))
    }

    pub fn wake(&self) {
        let shared = self.shared.lock();
        if shared.waker.is_some() {
            shared.waker.as_ref().unwrap().wake_by_ref();
        }
    }

    pub fn wake_write(&self) {
        let shared = self.shared.lock();
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
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut b = self.in_buf.lock();
        let len = b.len();
        if b.len() > 0 {
            if b.len() > buf.len() {
                buf.copy_from_slice(&b.drain(0..buf.len()).collect::<Vec<u8>>());
            } else {
                buf.copy_from_slice(&b);
                *b = vec![];
            }
            return Poll::Ready(Ok(len));
        }
        let mut shared = self.shared.lock();
        shared.waker = Some(cx.waker().to_owned());
        return Poll::Pending;
    }
}

// only used externally. always assume data frames and inject headers
impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if !self.ready.load(Ordering::Relaxed) {
            trace!("stream {} is not ready for writes", { self.id() });
            let mut shared = self.shared.lock();
            shared.write_waker = Some(cx.waker().to_owned());

            return Poll::Pending;
        }
        let header = MuxHeader::new(
            self.id,
            buf.len() as u32,
            Some(Flag::Syn),
            Some(FrameType::Data),
        );
        match self
            .writer
            .as_mut()
            .unwrap()
            .try_send((header, buf.to_vec()))
        {
            Ok(_) => Poll::Ready(Ok(buf.len() + HEADER_LENGTH)),
            Err(_) => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
