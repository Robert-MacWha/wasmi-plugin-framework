//! Non-blocking in-memory pipe implementation for WASI environment.
//!  
//! The pipe allows asynchronous and synchronous reads and writes,
//! avoiding deadlocks by returning the `WouldBlock` error when no data
//! is available for reading.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use futures::{AsyncRead, AsyncWrite};

use crate::wasi_ctx::WasiReader;

struct Inner {
    buf: VecDeque<u8>,
    closed: bool,
    waker: Option<Waker>,
}

#[derive(Clone)]
pub struct NonBlockingPipeReader {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Clone)]
pub struct NonBlockingPipeWriter {
    inner: Arc<Mutex<Inner>>,
}

pub fn non_blocking_pipe() -> (NonBlockingPipeReader, NonBlockingPipeWriter) {
    let inner = Arc::new(Mutex::new(Inner {
        buf: VecDeque::new(),
        closed: false,
        waker: None,
    }));
    (
        NonBlockingPipeReader {
            inner: inner.clone(),
        },
        NonBlockingPipeWriter { inner },
    )
}

impl Read for NonBlockingPipeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut inner = self.inner.lock().unwrap();

        let (front, back) = inner.buf.as_slices();
        let to_read = buf.len().min(front.len() + back.len());

        if to_read > 0 {
            let from_front = to_read.min(front.len());
            buf[..from_front].copy_from_slice(&front[..from_front]);

            if from_front < to_read {
                let from_back = to_read - from_front;
                buf[from_front..to_read].copy_from_slice(&back[..from_back]);
            }

            inner.buf.drain(..to_read);
            Ok(to_read)
        } else if inner.closed {
            Ok(0)
        } else {
            Err(io::ErrorKind::WouldBlock.into())
        }
    }
}

impl WasiReader for NonBlockingPipeReader {
    fn is_ready(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        !inner.buf.is_empty() || inner.closed
    }

    fn wait_ready(&self, _timeout: Option<std::time::Duration>) {
        // TODO: Make native-compatible version of this?
        // No-op
    }
}

impl AsyncRead for NonBlockingPipeReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let read = self.as_mut().get_mut().read(buf);
        match read {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                let mut inner = self.inner.lock().unwrap();
                inner.waker = Some(cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl Write for NonBlockingPipeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut inner = self.inner.lock().unwrap();
        inner.buf.extend(buf);
        if let Some(waker) = inner.waker.take() {
            waker.wake();
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl AsyncWrite for NonBlockingPipeWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut inner = self.inner.lock().unwrap();
        if inner.closed {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        inner.buf.extend(buf);
        if let Some(waker) = inner.waker.take() {
            waker.wake();
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl Drop for NonBlockingPipeWriter {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        inner.closed = true;
        if let Some(waker) = inner.waker.take() {
            waker.wake();
        }
    }
}
