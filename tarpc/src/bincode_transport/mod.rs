// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! A TCP [`Transport`] that serializes as bincode.

#![deny(missing_docs, missing_debug_implementations)]

use async_bincode::{AsyncBincodeStream, AsyncDestination};
use futures::{compat::*, prelude::*, ready, task::*};
use pin_project::pin_project;
use serde::{Deserialize, Serialize};
use std::{error::Error, io, marker::PhantomData, net::SocketAddr, pin::Pin};
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_tcp::{TcpListener, TcpStream};

/// A transport that serializes to, and deserializes from, a [`TcpStream`].
#[pin_project]
#[derive(Debug)]
pub struct Transport<S, Item, SinkItem> {
    #[pin]
    inner: Compat01As03Sink<AsyncBincodeStream<S, Item, SinkItem, AsyncDestination>, SinkItem>,
}

impl<S, Item, SinkItem> Stream for Transport<S, Item, SinkItem>
where
    S: AsyncRead,
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
    S: AsyncWrite,
    SinkItem: Serialize,
{
    type Error = io::Error;

    fn start_send(self: Pin<&mut Self>, item: SinkItem) -> io::Result<()> {
        self.project()
            .inner
            .start_send(item)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        convert(self.project().inner.poll_ready(cx))
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
    /// Returns the address of the peer connected over the transport.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().get_ref().peer_addr()
    }

    /// Returns the address of this end of the transport.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().get_ref().local_addr()
    }
}

impl<T, Item, SinkItem> AsRef<T> for Transport<T, Item, SinkItem> {
    fn as_ref(&self) -> &T {
        self.inner.get_ref().get_ref()
    }
}

/// Returns a new bincode transport that reads from and writes to `io`.
pub fn new<Item, SinkItem>(io: TcpStream) -> Transport<TcpStream, Item, SinkItem>
where
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
{
    Transport::from(io)
}

impl<S, Item, SinkItem> From<S> for Transport<S, Item, SinkItem> {
    fn from(inner: S) -> Self {
        Transport {
            inner: Compat01As03Sink::new(AsyncBincodeStream::from(inner).for_async()),
        }
    }
}

/// Connects to `addr`, wrapping the connection in a bincode transport.
pub async fn connect<Item, SinkItem>(
    addr: &SocketAddr,
) -> io::Result<Transport<TcpStream, Item, SinkItem>>
where
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
{
    Ok(new(TcpStream::connect(addr).compat().await?))
}

/// Listens on `addr`, wrapping accepted connections in bincode transports.
pub fn listen<Item, SinkItem>(addr: &SocketAddr) -> io::Result<Incoming<Item, SinkItem>>
where
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
{
    let listener = TcpListener::bind(addr)?;
    let local_addr = listener.local_addr()?;
    let incoming = listener.incoming().compat();
    Ok(Incoming {
        incoming,
        local_addr,
        ghost: PhantomData,
    })
}

/// A [`TcpListener`] that wraps connections in bincode transports.
#[pin_project]
#[derive(Debug)]
pub struct Incoming<Item, SinkItem> {
    #[pin]
    incoming: Compat01As03<tokio_tcp::Incoming>,
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
    use futures::{task::*, Sink, Stream};
    use pin_utils::pin_mut;
    use std::io::Cursor;

    fn ctx() -> Context<'static> {
        Context::from_waker(&noop_waker_ref())
    }

    #[test]
    fn test_stream() {
        // Frame is big endian; bincode is little endian. A bit confusing!
        let reader = *b"\x00\x00\x00\x1e\x16\x00\x00\x00\x00\x00\x00\x00Test one, check check.";
        let reader: Box<[u8]> = Box::new(reader);
        let transport = Transport::<_, String, String>::from(Cursor::new(reader));
        pin_mut!(transport);

        assert_matches!(
            transport.poll_next(&mut ctx()),
            Poll::Ready(Some(Ok(ref s))) if s == "Test one, check check.");
    }

    #[test]
    fn test_sink() {
        let writer: &mut [u8] = &mut [0; 34];
        let transport = Transport::<_, String, String>::from(Cursor::new(&mut *writer));
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
        assert_eq!(
            writer,
            <&[u8]>::from(
                b"\x00\x00\x00\x1e\x16\x00\x00\x00\x00\x00\x00\x00Test one, check check."
            )
        );
    }
}
