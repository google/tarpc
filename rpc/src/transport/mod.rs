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
    net::SocketAddr,
    pin::Pin,
    task::{LocalWaker, Poll},
};

pub mod channel;

/// A bidirectional stream ([`Sink`] + [`Stream`]) of messages.
pub trait Transport
where
    Self: Stream<Item = io::Result<<Self as Transport>::Item>>,
    Self: Sink<SinkItem = <Self as Transport>::SinkItem, SinkError = io::Error>,
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
pub fn new<S, Item>(
    inner: S,
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
) -> impl Transport<Item = Item, SinkItem = S::SinkItem>
where
    S: Stream<Item = io::Result<Item>>,
    S: Sink<SinkError = io::Error>,
{
    TransportShim {
        inner,
        peer_addr,
        local_addr,
    }
}

/// A transport created by adding peers to a Stream + Sink.
#[derive(Debug)]
struct TransportShim<S> {
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
    inner: S,
}

impl<S> TransportShim<S> {
    pin_utils::unsafe_pinned!(inner: S);
}

impl<S> Stream for TransportShim<S>
where
    S: Stream,
{
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, waker: &LocalWaker) -> Poll<Option<S::Item>> {
        self.inner().poll_next(waker)
    }
}

impl<S> Sink for TransportShim<S>
where
    S: Sink,
{
    type SinkItem = S::SinkItem;
    type SinkError = S::SinkError;

    fn start_send(mut self: Pin<&mut Self>, item: S::SinkItem) -> Result<(), S::SinkError> {
        self.inner().start_send(item)
    }

    fn poll_ready(mut self: Pin<&mut Self>, waker: &LocalWaker) -> Poll<Result<(), S::SinkError>> {
        self.inner().poll_ready(waker)
    }

    fn poll_flush(mut self: Pin<&mut Self>, waker: &LocalWaker) -> Poll<Result<(), S::SinkError>> {
        self.inner().poll_flush(waker)
    }

    fn poll_close(mut self: Pin<&mut Self>, waker: &LocalWaker) -> Poll<Result<(), S::SinkError>> {
        self.inner().poll_close(waker)
    }
}

impl<S, Item> Transport for TransportShim<S>
where
    S: Stream + Sink,
    Self: Stream<Item = io::Result<Item>>,
    Self: Sink<SinkItem = S::SinkItem, SinkError = io::Error>,
{
    type Item = Item;
    type SinkItem = S::SinkItem;

    /// The address of the remote peer this transport is in communication with.
    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer_addr)
    }

    /// The address of the local half of this transport.
    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local_addr)
    }
}
