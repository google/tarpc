// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a [`Transport`] trait as well as implementations.
//!
//! The rpc crate is transport- and protocol-agnostic. Any transport that impls [`Transport`]
//! can be plugged in, using whatever protocol it wants.

use futures::prelude::*;
use std::{
    io,
    marker::PhantomData,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};

pub mod channel;

/// A bidirectional stream ([`Sink`] + [`Stream`]) of messages.
pub trait Transport
where
    Self: Stream<Item = io::Result<<Self as Transport>::Item>>,
    Self: Sink<<Self as Transport>::SinkItem, Error = io::Error>,
{
    /// The type read off the transport.
    type Item;
    /// The type written to the transport.
    type SinkItem;

    /// The address of the remote peer this transport is in communication with.
    fn peer_addr(&self) -> io::Result<SocketAddr>;
    /// The address of the local half of this transport.
    fn local_addr(&self) -> io::Result<SocketAddr>;
}

/// Returns a new Transport backed by the given Stream + Sink and connecting addresses.
pub fn new<S, SinkItem, Item>(
    inner: S,
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
) -> impl Transport<Item = Item, SinkItem = SinkItem>
where
    S: Stream<Item = io::Result<Item>>,
    S: Sink<SinkItem, Error = io::Error>,
{
    TransportShim {
        inner,
        peer_addr,
        local_addr,
        _marker: PhantomData,
    }
}

/// A transport created by adding peers to a Stream + Sink.
#[derive(Debug)]
struct TransportShim<S, SinkItem> {
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
    inner: S,
    _marker: PhantomData<SinkItem>,
}

impl<S, SinkItem> TransportShim<S, SinkItem> {
    pin_utils::unsafe_pinned!(inner: S);
}

impl<S, SinkItem> Stream for TransportShim<S, SinkItem>
where
    S: Stream,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<S::Item>> {
        self.inner().poll_next(cx)
    }
}

impl<S, Item> Sink<Item> for TransportShim<S, Item>
where
    S: Sink<Item>,
{
    type Error = S::Error;

    fn start_send(self: Pin<&mut Self>, item: Item) -> Result<(), S::Error> {
        self.inner().start_send(item)
    }

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner().poll_ready(cx)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner().poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner().poll_close(cx)
    }
}

impl<S, SinkItem, Item> Transport for TransportShim<S, SinkItem>
where
    S: Stream + Sink<SinkItem>,
    Self: Stream<Item = io::Result<Item>>,
    Self: Sink<SinkItem, Error = io::Error>,
{
    type Item = Item;
    type SinkItem = SinkItem;

    /// The address of the remote peer this transport is in communication with.
    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer_addr)
    }

    /// The address of the local half of this transport.
    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local_addr)
    }
}
