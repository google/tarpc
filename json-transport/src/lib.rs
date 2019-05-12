// Copyright 2019 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! A TCP [`Transport`] that serializes as JSON.

#![feature(arbitrary_self_types, async_await)]
#![deny(missing_docs)]

use futures::{compat::*, prelude::*, ready};
use pin_utils::unsafe_pinned;
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
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_serde_json::*;
use tokio_tcp::{TcpListener, TcpStream};

/// A transport that serializes to, and deserializes from, a [`TcpStream`].
pub struct Transport<S: AsyncWrite, Item, SinkItem> {
    inner: Compat01As03Sink<
        ReadJson<WriteJson<Framed<S, LengthDelimitedCodec>, SinkItem>, Item>,
        SinkItem,
    >,
}

impl<S: AsyncWrite, Item, SinkItem> Transport<S, Item, SinkItem> {
    unsafe_pinned!(
        inner:
            Compat01As03Sink<
                ReadJson<WriteJson<Framed<S, LengthDelimitedCodec>, SinkItem>, Item>,
                SinkItem,
            >
    );
}

impl<S, Item, SinkItem> Stream for Transport<S, Item, SinkItem>
where
    S: AsyncWrite + AsyncRead,
    Item: for<'a> Deserialize<'a>,
{
    type Item = io::Result<Item>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<io::Result<Item>>> {
        match self.inner().poll_next(cx) {
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
    type SinkError = io::Error;

    fn start_send(self: Pin<&mut Self>, item: SinkItem) -> io::Result<()> {
        self.inner()
            .start_send(item)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        convert(self.inner().poll_ready(cx))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        convert(self.inner().poll_flush(cx))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        convert(self.inner().poll_close(cx))
    }
}

fn convert<E: Into<Box<Error + Send + Sync>>>(poll: Poll<Result<(), E>>) -> Poll<io::Result<()>> {
    match poll {
        Poll::Pending => Poll::Pending,
        Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
        Poll::Ready(Err(e)) => Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e))),
    }
}

impl<Item, SinkItem> Transport<TcpStream, Item, SinkItem> {
    /// Returns the peer address of the underlying TcpStream.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner
            .get_ref()
            .get_ref()
            .get_ref()
            .get_ref()
            .peer_addr()
    }

    /// Returns the local address of the underlying TcpStream.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner
            .get_ref()
            .get_ref()
            .get_ref()
            .get_ref()
            .local_addr()
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
            inner: Compat01As03Sink::new(ReadJson::new(WriteJson::new(Framed::new(
                inner,
                LengthDelimitedCodec::new(),
            )))),
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
    Ok(new(TcpStream::connect(addr).compat().await?))
}

/// Listens on `addr`, wrapping accepted connections in JSON transports.
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

/// A [`TcpListener`] that wraps connections in JSON transports.
#[derive(Debug)]
pub struct Incoming<Item, SinkItem> {
    incoming: Compat01As03<tokio_tcp::Incoming>,
    local_addr: SocketAddr,
    ghost: PhantomData<(Item, SinkItem)>,
}

impl<Item, SinkItem> Incoming<Item, SinkItem> {
    unsafe_pinned!(incoming: Compat01As03<tokio_tcp::Incoming>);

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
        let next = ready!(self.incoming().poll_next(cx)?);
        Poll::Ready(next.map(|conn| Ok(new(conn))))
    }
}
