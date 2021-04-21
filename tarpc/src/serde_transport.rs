// Copyright 2019 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! A generic Serde-based `Transport` that can serialize anything supported by `tokio-serde` via any medium that implements `AsyncRead` and `AsyncWrite`.

#![deny(missing_docs)]

use futures::{prelude::*, task::*};
use pin_project::pin_project;
use serde::{Deserialize, Serialize};
use std::{error::Error, io, pin::Pin};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_serde::{Framed as SerdeFramed, *};
use tokio_util::codec::{length_delimited::LengthDelimitedCodec, Framed};

/// A transport that serializes to, and deserializes from, a byte stream.
#[pin_project]
pub struct Transport<S, Item, SinkItem, Codec> {
    #[pin]
    inner: SerdeFramed<Framed<S, LengthDelimitedCodec>, Item, SinkItem, Codec>,
}

impl<S, Item, SinkItem, Codec> Transport<S, Item, SinkItem, Codec> {
    /// Returns the inner transport over which messages are sent and received.
    pub fn get_ref(&self) -> &S {
        self.inner.get_ref().get_ref()
    }
}

impl<S, Item, SinkItem, Codec, CodecError> Stream for Transport<S, Item, SinkItem, Codec>
where
    S: AsyncWrite + AsyncRead,
    Item: for<'a> Deserialize<'a>,
    Codec: Deserializer<Item>,
    CodecError: Into<Box<dyn std::error::Error + Send + Sync>>,
    SerdeFramed<Framed<S, LengthDelimitedCodec>, Item, SinkItem, Codec>:
        Stream<Item = Result<Item, CodecError>>,
{
    type Item = io::Result<Item>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<io::Result<Item>>> {
        self.project()
            .inner
            .poll_next(cx)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }
}

impl<S, Item, SinkItem, Codec, CodecError> Sink<SinkItem> for Transport<S, Item, SinkItem, Codec>
where
    S: AsyncWrite,
    SinkItem: Serialize,
    Codec: Serializer<SinkItem>,
    CodecError: Into<Box<dyn Error + Send + Sync>>,
    SerdeFramed<Framed<S, LengthDelimitedCodec>, Item, SinkItem, Codec>:
        Sink<SinkItem, Error = CodecError>,
{
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project()
            .inner
            .poll_ready(cx)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    fn start_send(self: Pin<&mut Self>, item: SinkItem) -> io::Result<()> {
        self.project()
            .inner
            .start_send(item)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project()
            .inner
            .poll_flush(cx)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project()
            .inner
            .poll_close(cx)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }
}

/// Constructs a new transport from a framed transport and a serialization codec.
pub fn new<S, Item, SinkItem, Codec>(
    framed_io: Framed<S, LengthDelimitedCodec>,
    codec: Codec,
) -> Transport<S, Item, SinkItem, Codec>
where
    S: AsyncWrite + AsyncRead,
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
    Codec: Serializer<SinkItem> + Deserializer<Item>,
{
    Transport {
        inner: SerdeFramed::new(framed_io, codec),
    }
}

impl<S, Item, SinkItem, Codec> From<(S, Codec)> for Transport<S, Item, SinkItem, Codec>
where
    S: AsyncWrite + AsyncRead,
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
    Codec: Serializer<SinkItem> + Deserializer<Item>,
{
    fn from((io, codec): (S, Codec)) -> Self {
        new(Framed::new(io, LengthDelimitedCodec::new()), codec)
    }
}

#[cfg(feature = "tcp")]
#[cfg_attr(docsrs, doc(cfg(feature = "tcp")))]
/// TCP support for generic transport using Tokio.
pub mod tcp {
    use {
        super::*,
        futures::ready,
        std::{marker::PhantomData, net::SocketAddr},
        tokio::net::{TcpListener, TcpStream, ToSocketAddrs},
        tokio_util::codec::length_delimited,
    };

    mod private {
        use super::*;

        pub trait Sealed {}

        impl<Item, SinkItem, Codec> Sealed for Transport<TcpStream, Item, SinkItem, Codec> {}
    }

    impl<Item, SinkItem, Codec> Transport<TcpStream, Item, SinkItem, Codec> {
        /// Returns the peer address of the underlying TcpStream.
        pub fn peer_addr(&self) -> io::Result<SocketAddr> {
            self.inner.get_ref().get_ref().peer_addr()
        }
        /// Returns the local address of the underlying TcpStream.
        pub fn local_addr(&self) -> io::Result<SocketAddr> {
            self.inner.get_ref().get_ref().local_addr()
        }
    }

    /// A connection Future that also exposes the length-delimited framing config.
    #[pin_project]
    pub struct Connect<T, Item, SinkItem, CodecFn> {
        #[pin]
        inner: T,
        codec_fn: CodecFn,
        config: length_delimited::Builder,
        ghost: PhantomData<(fn(SinkItem), fn() -> Item)>,
    }

    impl<T, Item, SinkItem, Codec, CodecFn> Future for Connect<T, Item, SinkItem, CodecFn>
    where
        T: Future<Output = io::Result<TcpStream>>,
        Item: for<'de> Deserialize<'de>,
        SinkItem: Serialize,
        Codec: Serializer<SinkItem> + Deserializer<Item>,
        CodecFn: Fn() -> Codec,
    {
        type Output = io::Result<Transport<TcpStream, Item, SinkItem, Codec>>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            let io = ready!(self.as_mut().project().inner.poll(cx))?;
            Poll::Ready(Ok(new(self.config.new_framed(io), (self.codec_fn)())))
        }
    }

    impl<T, Item, SinkItem, CodecFn> Connect<T, Item, SinkItem, CodecFn> {
        /// Returns an immutable reference to the length-delimited codec's config.
        pub fn config(&self) -> &length_delimited::Builder {
            &self.config
        }

        /// Returns a mutable reference to the length-delimited codec's config.
        pub fn config_mut(&mut self) -> &mut length_delimited::Builder {
            &mut self.config
        }
    }

    /// Connects to `addr`, wrapping the connection in a TCP transport.
    pub fn connect<A, Item, SinkItem, Codec, CodecFn>(
        addr: A,
        codec_fn: CodecFn,
    ) -> Connect<impl Future<Output = io::Result<TcpStream>>, Item, SinkItem, CodecFn>
    where
        A: ToSocketAddrs,
        Item: for<'de> Deserialize<'de>,
        SinkItem: Serialize,
        Codec: Serializer<SinkItem> + Deserializer<Item>,
        CodecFn: Fn() -> Codec,
    {
        Connect {
            inner: TcpStream::connect(addr),
            codec_fn,
            config: LengthDelimitedCodec::builder(),
            ghost: PhantomData,
        }
    }

    /// Listens on `addr`, wrapping accepted connections in TCP transports.
    pub async fn listen<A, Item, SinkItem, Codec, CodecFn>(
        addr: A,
        codec_fn: CodecFn,
    ) -> io::Result<Incoming<Item, SinkItem, Codec, CodecFn>>
    where
        A: ToSocketAddrs,
        Item: for<'de> Deserialize<'de>,
        Codec: Serializer<SinkItem> + Deserializer<Item>,
        CodecFn: Fn() -> Codec,
    {
        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        Ok(Incoming {
            listener,
            codec_fn,
            local_addr,
            config: LengthDelimitedCodec::builder(),
            ghost: PhantomData,
        })
    }

    /// A [`TcpListener`] that wraps connections in [transports](Transport).
    #[pin_project]
    #[derive(Debug)]
    pub struct Incoming<Item, SinkItem, Codec, CodecFn> {
        listener: TcpListener,
        local_addr: SocketAddr,
        codec_fn: CodecFn,
        config: length_delimited::Builder,
        ghost: PhantomData<(fn() -> Item, fn(SinkItem), Codec)>,
    }

    impl<Item, SinkItem, Codec, CodecFn> Incoming<Item, SinkItem, Codec, CodecFn> {
        /// Returns the address being listened on.
        pub fn local_addr(&self) -> SocketAddr {
            self.local_addr
        }

        /// Returns an immutable reference to the length-delimited codec's config.
        pub fn config(&self) -> &length_delimited::Builder {
            &self.config
        }

        /// Returns a mutable reference to the length-delimited codec's config.
        pub fn config_mut(&mut self) -> &mut length_delimited::Builder {
            &mut self.config
        }
    }

    impl<Item, SinkItem, Codec, CodecFn> Stream for Incoming<Item, SinkItem, Codec, CodecFn>
    where
        Item: for<'de> Deserialize<'de>,
        SinkItem: Serialize,
        Codec: Serializer<SinkItem> + Deserializer<Item>,
        CodecFn: Fn() -> Codec,
    {
        type Item = io::Result<Transport<TcpStream, Item, SinkItem, Codec>>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let conn: TcpStream =
                ready!(Pin::new(&mut self.as_mut().project().listener).poll_accept(cx)?).0;
            Poll::Ready(Some(Ok(new(
                self.config.new_framed(conn),
                (self.codec_fn)(),
            ))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Transport;
    use assert_matches::assert_matches;
    use futures::{task::*, Sink, Stream};
    use pin_utils::pin_mut;
    use std::{
        io::{self, Cursor},
        pin::Pin,
    };
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio_serde::formats::SymmetricalJson;

    fn ctx() -> Context<'static> {
        Context::from_waker(&noop_waker_ref())
    }

    struct TestIo(Cursor<Vec<u8>>);

    impl AsyncRead for TestIo {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            AsyncRead::poll_read(Pin::new(&mut self.0), cx, buf)
        }
    }

    impl AsyncWrite for TestIo {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            AsyncWrite::poll_write(Pin::new(&mut self.0), cx, buf)
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            AsyncWrite::poll_flush(Pin::new(&mut self.0), cx)
        }

        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            AsyncWrite::poll_shutdown(Pin::new(&mut self.0), cx)
        }
    }

    #[test]
    fn close() {
        let (tx, _rx) = crate::transport::channel::bounded::<(), ()>(0);
        pin_mut!(tx);
        assert_matches!(tx.as_mut().poll_close(&mut ctx()), Poll::Ready(Ok(())));
        assert_matches!(tx.as_mut().start_send(()), Err(_));
    }

    #[test]
    fn test_stream() {
        let data: &[u8] = b"\x00\x00\x00\x18\"Test one, check check.\"";
        let transport = Transport::from((
            TestIo(Cursor::new(Vec::from(data))),
            SymmetricalJson::<String>::default(),
        ));
        pin_mut!(transport);

        assert_matches!(
            transport.as_mut().poll_next(&mut ctx()),
            Poll::Ready(Some(Ok(ref s))) if s == "Test one, check check.");
        assert_matches!(transport.as_mut().poll_next(&mut ctx()), Poll::Ready(None));
    }

    #[test]
    fn test_sink() {
        let writer = Cursor::new(vec![]);
        let mut transport = Box::pin(Transport::from((
            TestIo(writer),
            SymmetricalJson::<String>::default(),
        )));

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
        assert_matches!(
            transport.as_mut().poll_flush(&mut ctx()),
            Poll::Ready(Ok(()))
        );
        assert_eq!(
            transport.get_ref().0.get_ref(),
            b"\x00\x00\x00\x18\"Test one, check check.\""
        );
    }

    #[cfg(tcp)]
    #[tokio::test]
    async fn tcp() -> io::Result<()> {
        use super::tcp;

        let mut listener = tcp::listen("0.0.0.0:0", SymmetricalJson::<String>::default).await?;
        let addr = listener.local_addr();
        tokio::spawn(async move {
            let mut transport = listener.next().await.unwrap().unwrap();
            let message = transport.next().await.unwrap().unwrap();
            transport.send(message).await.unwrap();
        });
        let mut transport = tcp::connect(addr, SymmetricalJson::<String>::default).await?;
        transport.send(String::from("test")).await?;
        assert_matches!(transport.next().await, Some(Ok(s)) if s == "test");
        assert_matches!(transport.next().await, None);
        Ok(())
    }
}
