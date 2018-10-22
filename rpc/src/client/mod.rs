// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a client that connects to a server and sends multiplexed requests.

use crate::{context, ClientMessage, Response, Transport};
use futures::Future;
use log::warn;
use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
};

mod dispatch;

/// Sends multiplexed requests to, and receives responses from, a server.
pub trait Client<Req, Resp> {
    /// The future response from the server.
    type Response: Future<Output = io::Result<Resp>>;

    /// Initiates a request, sending it to the dispatch task.
    ///
    /// Returns a [`Future`] that resolves to this client and the future response
    /// once the request is successfully enqueued.
    ///
    /// [`Future`]: futures::Future
    fn call(self, ctx: context::Context, request: Req) -> Self::Response;
}

impl<'a, Req, Resp> Client<Req, Resp> for &'a mut dispatch::Channel<Req, Resp> {
    type Response = dispatch::Call<'a, Req, Resp>;

    fn call(self, ctx: context::Context, request: Req) -> dispatch::Call<'a, Req, Resp> {
        self.call(ctx, request)
    }
}


/// Settings that control the behavior of the client.
#[non_exhaustive]
#[derive(Clone, Debug)]
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

/// Creates a new Client by wrapping a [`Transport`] and spawning a dispatch task
/// that manages the lifecycle of requests.
///
/// Must only be called from on an executor.
pub async fn new<Req, Resp, T>(config: Config, transport: T) -> io::Result<dispatch::Channel<Req, Resp>>
where
    Req: Send,
    Resp: Send,
    T: Transport<Item = Response<Resp>, SinkItem = ClientMessage<Req>> + Send,
{
    let server_addr = transport.peer_addr().unwrap_or_else(|e| {
        warn!(
            "Setting peer to unspecified because peer could not be determined: {}",
            e
        );
        SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0)
    });

    Ok(await!(dispatch::spawn(config, transport, server_addr))?)
}

