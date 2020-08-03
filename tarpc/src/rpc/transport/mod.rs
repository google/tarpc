// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a [`Transport`](sealed::Transport) trait as well as implementations.
//!
//! The rpc crate is transport- and protocol-agnostic. Any transport that impls [`Transport`](sealed::Transport)
//! can be plugged in, using whatever protocol it wants.

use futures::prelude::*;
use std::io;

pub mod channel;

pub(crate) mod sealed {
    use super::*;

    /// A bidirectional stream ([`Sink`] + [`Stream`]) of messages.
    pub trait Transport<SinkItem, Item>:
        Stream<Item = io::Result<Item>> + Sink<SinkItem, Error = io::Error>
    {
    }

    impl<T, SinkItem, Item> Transport<SinkItem, Item> for T where
        T: Stream<Item = io::Result<Item>> + Sink<SinkItem, Error = io::Error> + ?Sized
    {
    }
}
