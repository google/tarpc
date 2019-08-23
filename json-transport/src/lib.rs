// Copyright 2019 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! A TCP [`Transport`] that serializes as JSON.

#![deny(missing_docs)]

use futures::{prelude::*, ready};
use pin_project::pin_project;
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    io,
    marker::PhantomData,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::codec::{length_delimited::LengthDelimitedCodec, Framed};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio_net::ToSocketAddrs;
use tokio_serde_json::*;

/// A transport that serializes to, and deserializes from, a [`TcpStream`].
#[pin_project]
pub struct Transport<S, Item, SinkItem> {
    #[pin]
    inner: ReadJson<WriteJson<Framed<S, LengthDelimitedCodec>, SinkItem>, Item>,
}

impl<S, Item, SinkItem> Stream for Transport<S, Item, SinkItem>
where
    S: AsyncWrite + AsyncRead + Unpin,
    Item: for<'a> Deserialize<'a>,
{
    type Item = io::Result<Item>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<io::Result<Item>>> {
        match self.project().inner.poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(Ok(next))) => Poll::Ready(Some(Ok(next))),
            Poll::Ready(Some(Err(e))) => {
                Poll::Ready(Some(Err(io::Error::new(io::ErrorKind::Other, e))))
            }
        }
    }
}

impl<S, Item, SinkItem> Sink<SinkItem> for Transport<S, Item, SinkItem>
where
    S: AsyncWrite + Unpin,
    SinkItem: Serialize,
{
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        convert(self.project().inner.poll_ready(cx))
    }

    fn start_send(self: Pin<&mut Self>, item: SinkItem) -> io::Result<()> {
        self.project()
            .inner
            .start_send(item)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        convert(self.project().inner.poll_flush(cx))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        convert(self.project().inner.poll_close(cx))
    }
}

fn convert<E: Into<Box<dyn Error + Send + Sync>>>(
    poll: Poll<Result<(), E>>,
) -> Poll<io::Result<()>> {
    match poll {
        Poll::Pending => Poll::Pending,
        Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
        Poll::Ready(Err(e)) => Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e))),
    }
}

impl<Item, SinkItem> Transport<TcpStream, Item, SinkItem> {
    /// Returns the peer address of the underlying TcpStream.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().get_ref().get_ref().peer_addr()
    }

    /// Returns the local address of the underlying TcpStream.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().get_ref().get_ref().local_addr()
    }
}

/// Returns a new JSON transport that reads from and writes to `io`.
pub fn new<Item, SinkItem>(io: TcpStream) -> Transport<TcpStream, Item, SinkItem>
where
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
{
    Transport::from(io)
}

impl<S: AsyncWrite + AsyncRead, Item: serde::de::DeserializeOwned, SinkItem: Serialize> From<S>
    for Transport<S, Item, SinkItem>
{
    fn from(inner: S) -> Self {
        Transport {
            inner: ReadJson::new(WriteJson::new(Framed::new(
                inner,
                LengthDelimitedCodec::new(),
            ))),
        }
    }
}

/// Connects to `addr`, wrapping the connection in a JSON transport.
pub async fn connect<Item, SinkItem>(
    addr: &SocketAddr,
) -> io::Result<Transport<TcpStream, Item, SinkItem>>
where
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
{
    Ok(new(TcpStream::connect(addr).await?))
}

/// Listens on `addr`, wrapping accepted connections in JSON transports.
pub async fn listen<A, Item, SinkItem>(addr: A) -> io::Result<Incoming<Item, SinkItem>>
where
    A: ToSocketAddrs,
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
{
    let listener = TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;
    let incoming = Box::pin(listener.incoming());
    Ok(Incoming {
        incoming,
        local_addr,
        ghost: PhantomData,
    })
}

trait IncomingTrait: Stream<Item = io::Result<TcpStream>> + std::fmt::Debug + Send {}
impl<T: Stream<Item = io::Result<TcpStream>> + std::fmt::Debug + Send> IncomingTrait for T {}

/// A [`TcpListener`] that wraps connections in JSON transports.
#[pin_project]
#[derive(Debug)]
pub struct Incoming<Item, SinkItem> {
    #[pin]
    incoming: Pin<Box<dyn IncomingTrait>>,
    local_addr: SocketAddr,
    ghost: PhantomData<(Item, SinkItem)>,
}

impl<Item, SinkItem> Incoming<Item, SinkItem> {
    /// Returns the address being listened on.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl<Item, SinkItem> Stream for Incoming<Item, SinkItem>
where
    Item: for<'a> Deserialize<'a>,
    SinkItem: Serialize,
{
    type Item = io::Result<Transport<TcpStream, Item, SinkItem>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let next = ready!(self.project().incoming.poll_next(cx)?);
        Poll::Ready(next.map(|conn| Ok(new(conn))))
    }
}

#[cfg(test)]
mod tests {
    use super::Transport;
    use assert_matches::assert_matches;
    use futures::task::noop_waker_ref;
    use futures::{Sink, Stream};
    use pin_utils::pin_mut;
    use std::{
        io::{self, Cursor},
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::{AsyncRead, AsyncWrite};

    fn ctx() -> Context<'static> {
        Context::from_waker(&noop_waker_ref())
    }

    #[test]
    fn test_stream() {
        struct TestIo(Cursor<&'static [u8]>);

        impl AsyncRead for TestIo {
            fn poll_read(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &mut [u8],
            ) -> Poll<io::Result<usize>> {
                AsyncRead::poll_read(Pin::new(self.0.get_mut()), cx, buf)
            }
        }

        impl AsyncWrite for TestIo {
            fn poll_write(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                _buf: &[u8],
            ) -> Poll<io::Result<usize>> {
                unreachable!()
            }

            fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                unreachable!()
            }

            fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                unreachable!()
            }
        }

        let data = b"\x00\x00\x00\x18\"Test one, check check.\"";
        let transport = Transport::<_, String, String>::from(TestIo(Cursor::new(data)));
        pin_mut!(transport);

        assert_matches!(
            transport.poll_next(&mut ctx()),
            Poll::Ready(Some(Ok(ref s))) if s == "Test one, check check.");
    }

    #[test]
    fn test_sink() {
        struct TestIo<'a>(&'a mut Vec<u8>);

        impl<'a> AsyncRead for TestIo<'a> {
            fn poll_read(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                _buf: &mut [u8],
            ) -> Poll<io::Result<usize>> {
                unreachable!()
            }
        }

        impl<'a> AsyncWrite for TestIo<'a> {
            fn poll_write(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<io::Result<usize>> {
                AsyncWrite::poll_write(Pin::new(&mut *self.0), cx, buf)
            }

            fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                AsyncWrite::poll_flush(Pin::new(&mut *self.0), cx)
            }

            fn poll_shutdown(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<io::Result<()>> {
                AsyncWrite::poll_shutdown(Pin::new(&mut *self.0), cx)
            }
        }

        let mut writer = vec![];
        let transport = Transport::<_, String, String>::from(TestIo(&mut writer));
        pin_mut!(transport);

        assert_matches!(
            transport.as_mut().poll_ready(&mut ctx()),
            Poll::Ready(Ok(()))
        );
        assert_matches!(
            transport
                .as_mut()
                .start_send("Test one, check check.".into()),
            Ok(())
        );
        assert_matches!(transport.poll_flush(&mut ctx()), Poll::Ready(Ok(())));
        assert_eq!(writer, b"\x00\x00\x00\x18\"Test one, check check.\"");
    }
}
