use std::pin::Pin;

use iroh::endpoint::{RecvStream, SendStream};
use tokio::io::{AsyncRead, AsyncWrite};

pub struct TxRx {
    pub(crate) send: SendStream,
    pub(crate) recv: RecvStream,
}

impl TxRx {
    pub fn new(send: SendStream, recv: RecvStream) -> Self {
        Self { send, recv }
    }
}

impl AsyncWrite for TxRx {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        let mut_self = self.get_mut();
        tokio::io::AsyncWrite::poll_write(Pin::new(&mut mut_self.send), cx, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let mut_self = self.get_mut();
        tokio::io::AsyncWrite::poll_flush(Pin::new(&mut mut_self.send), cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let mut_self = self.get_mut();
        tokio::io::AsyncWrite::poll_shutdown(Pin::new(&mut mut_self.send), cx)
    }
}

impl AsyncRead for TxRx {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let mut_self = self.get_mut();
        tokio::io::AsyncRead::poll_read(Pin::new(&mut mut_self.recv), cx, buf)
    }
}
