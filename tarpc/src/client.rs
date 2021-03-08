// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a client that connects to a server and sends multiplexed requests.

use futures::prelude::*;
use std::fmt;
use std::io;

/// Provides a [`Client`] backed by a transport.
pub mod channel;
mod in_flight_requests;
pub use channel::{new, Channel};

/// Settings that control the behavior of the client.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Config {
    /// The number of requests that can be in flight at once.
    /// `max_in_flight_requests` controls the size of the map used by the client
    /// for storing pending requests.
    pub max_in_flight_requests: usize,
    /// The number of requests that can be buffered client-side before being sent.
    /// `pending_requests_buffer` controls the size of the channel clients use
    /// to communicate with the request dispatch task.
    pub pending_request_buffer: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_in_flight_requests: 1_000,
            pending_request_buffer: 100,
        }
    }
}

/// A channel and dispatch pair. The dispatch drives the sending and receiving of requests
/// and must be polled continuously or spawned.
pub struct NewClient<C, D> {
    /// The new client.
    pub client: C,
    /// The client's dispatch.
    pub dispatch: D,
}

impl<C, D, E> NewClient<C, D>
where
    D: Future<Output = Result<(), E>> + Send + 'static,
    E: std::fmt::Display,
{
    /// Helper method to spawn the dispatch on the default executor.
    #[cfg(feature = "tokio1")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tokio1")))]
    pub fn spawn(self) -> io::Result<C> {
        use log::warn;

        let dispatch = self
            .dispatch
            .unwrap_or_else(move |e| warn!("Connection broken: {}", e));
        tokio::spawn(dispatch);
        Ok(self.client)
    }
}

impl<C, D> fmt::Debug for NewClient<C, D> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "NewClient")
    }
}
