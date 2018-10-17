// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a client that connects to a server and sends multiplexed requests.

use crate::{context::Context, ClientMessage, Response, Transport};
use log::warn;
use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
};

mod dispatch;

/// Sends multiplexed requests to, and receives responses from, a server.
#[derive(Debug)]
pub struct Client<Req, Resp> {
    /// Channel to send requests to the dispatch task.
    channel: dispatch::Channel<Req, Resp>,
}

impl<Req, Resp> Clone for Client<Req, Resp> {
    fn clone(&self) -> Self {
        Client {
            channel: self.channel.clone(),
        }
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

impl<Req, Resp> Client<Req, Resp>
where
    Req: Send,
    Resp: Send,
{
    /// Creates a new Client by wrapping a [`Transport`] and spawning a dispatch task
    /// that manages the lifecycle of requests.
    ///
    /// Must only be called from on an executor.
    pub async fn new<T>(config: Config, transport: T) -> io::Result<Self>
    where
        T: Transport<Item = Response<Resp>, SinkItem = ClientMessage<Req>> + Send,
    {
        let server_addr = transport.peer_addr().unwrap_or_else(|e| {
            warn!(
                "Setting peer to unspecified because peer could not be determined: {}",
                e
            );
            SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0)
        });

        Ok(Client {
            channel: await!(dispatch::spawn(config, transport, server_addr))?,
        })
    }

    /// Initiates a request, sending it to the dispatch task.
    ///
    /// Returns a [`Future`] that resolves to this client and the future response
    /// once the request is successfully enqueued.
    ///
    /// [`Future`]: futures::Future
    pub async fn call(&mut self, ctx: Context, request: Req) -> io::Result<Resp> {
        await!(self.channel.call(ctx, request))
    }
}
