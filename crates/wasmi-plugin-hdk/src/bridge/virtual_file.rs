use std::fmt::Debug;
use std::io::SeekFrom;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite, ReadBuf};
use wasmer_wasix::virtual_fs::{Result as FsResult, VirtualFile};

pub struct WasiInputFile<R> {
    pub reader: R,
}

pub struct WasiOutputFile<W> {
    pub writer: W,
}

impl<R> Debug for WasiInputFile<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasiInputFile").finish()
    }
}

impl<W> Debug for WasiOutputFile<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasiOutputFile").finish()
    }
}

impl<R> VirtualFile for WasiInputFile<R>
where
    R: futures::AsyncRead + Send + Sync + Unpin + 'static,
{
    fn last_accessed(&self) -> u64 {
        0
    }
    fn last_modified(&self) -> u64 {
        0
    }
    fn created_time(&self) -> u64 {
        0
    }
    fn size(&self) -> u64 {
        0
    }
    fn set_len(&mut self, _: u64) -> FsResult<()> {
        Ok(())
    }
    fn unlink(&mut self) -> FsResult<()> {
        Ok(())
    }

    fn poll_read_ready(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Ok(0))
    }

    fn poll_write_ready(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(std::io::ErrorKind::Unsupported.into()))
    }
}

impl<R: futures::AsyncRead + Unpin> AsyncRead for WasiInputFile<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let inner_buf = buf.initialize_unfilled();
        match Pin::new(&mut self.reader).poll_read(cx, inner_buf) {
            Poll::Ready(Ok(n)) => {
                buf.advance(n);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<R> AsyncSeek for WasiInputFile<R> {
    fn start_seek(self: Pin<&mut Self>, _position: SeekFrom) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Pipes are not seekable",
        ))
    }

    fn poll_complete(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
        Poll::Ready(Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Pipes are not seekable",
        )))
    }
}

impl<R> AsyncWrite for WasiInputFile<R> {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(std::io::ErrorKind::Unsupported.into()))
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl<W> VirtualFile for WasiOutputFile<W>
where
    W: std::io::Write + Send + Sync + Unpin + 'static,
{
    fn last_accessed(&self) -> u64 {
        0
    }
    fn last_modified(&self) -> u64 {
        0
    }
    fn created_time(&self) -> u64 {
        0
    }
    fn size(&self) -> u64 {
        0
    }
    fn set_len(&mut self, _: u64) -> FsResult<()> {
        Ok(())
    }
    fn unlink(&mut self) -> FsResult<()> {
        Ok(())
    }

    fn poll_read_ready(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(std::io::ErrorKind::Unsupported.into()))
    }

    fn poll_write_ready(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Ok(0))
    }
}

impl<W: std::io::Write + Unpin> AsyncWrite for WasiOutputFile<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(self.writer.write(buf))
    }
    fn poll_flush(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(self.writer.flush())
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl<W> AsyncSeek for WasiOutputFile<W> {
    fn start_seek(self: Pin<&mut Self>, _position: SeekFrom) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Pipes are not seekable",
        ))
    }
    fn poll_complete(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
        Poll::Ready(Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Pipes are not seekable",
        )))
    }
}

impl<W> AsyncRead for WasiOutputFile<W> {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::ErrorKind::Unsupported.into()))
    }
}
