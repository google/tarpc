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
use std::{io, net::SocketAddr};

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
