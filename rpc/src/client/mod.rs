// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a client that connects to a server and sends multiplexed requests.

use crate::{context, ClientMessage, Response, Transport};
use futures::prelude::*;
use std::io;

/// Provides a [`Client`] backed by a transport.
pub mod channel;
pub use self::channel::Channel;

/// Sends multiplexed requests to, and receives responses from, a server.
pub trait Client<'a, Req> {
    /// The response type.
    type Response;

    /// The future response.
    type Future: Future<Output = io::Result<Self::Response>> + 'a;

    /// Initiates a request, sending it to the dispatch task.
    ///
    /// Returns a [`Future`] that resolves to this client and the future response
    /// once the request is successfully enqueued.
    ///
    /// [`Future`]: futures::Future
    fn call(&'a mut self, ctx: context::Context, request: Req) -> Self::Future;

    /// Returns a Client that applies a post-processing function to the returned response.
    fn map_response<F, R>(self, f: F) -> MapResponse<Self, F>
    where
        F: FnMut(Self::Response) -> R,
        Self: Sized,
    {
        MapResponse { inner: self, f }
    }

    /// Returns a Client that applies a pre-processing function to the request.
    fn with_request<F, Req2>(self, f: F) -> WithRequest<Self, F>
    where
        F: FnMut(Req2) -> Req,
        Self: Sized,
    {
        WithRequest { inner: self, f }
    }
}

/// A Client that applies a function to the returned response.
#[derive(Clone, Debug)]
pub struct MapResponse<C, F> {
    inner: C,
    f: F,
}

impl<'a, C, F, Req, Resp, Resp2> Client<'a, Req> for MapResponse<C, F>
where
    C: Client<'a, Req, Response = Resp>,
    F: FnMut(Resp) -> Resp2 + 'a,
{
    type Response = Resp2;
    type Future = futures::future::MapOk<<C as Client<'a, Req>>::Future, &'a mut F>;

    fn call(&'a mut self, ctx: context::Context, request: Req) -> Self::Future {
        self.inner.call(ctx, request).map_ok(&mut self.f)
    }
}

/// A Client that applies a pre-processing function to the request.
#[derive(Clone, Debug)]
pub struct WithRequest<C, F> {
    inner: C,
    f: F,
}

impl<'a, C, F, Req, Req2, Resp> Client<'a, Req2> for WithRequest<C, F>
where
    C: Client<'a, Req, Response = Resp>,
    F: FnMut(Req2) -> Req,
{
    type Response = Resp;
    type Future = <C as Client<'a, Req>>::Future;

    fn call(&'a mut self, ctx: context::Context, request: Req2) -> Self::Future {
        self.inner.call(ctx, (self.f)(request))
    }
}

impl<'a, Req, Resp> Client<'a, Req> for Channel<Req, Resp>
where
    Req: 'a,
    Resp: 'a,
{
    type Response = Resp;
    type Future = channel::Call<'a, Req, Resp>;

    fn call(&'a mut self, ctx: context::Context, request: Req) -> channel::Call<'a, Req, Resp> {
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
pub async fn new<Req, Resp, T>(config: Config, transport: T) -> io::Result<Channel<Req, Resp>>
where
    Req: Send + 'static,
    Resp: Send + 'static,
    T: Transport<ClientMessage<Req>, Response<Resp>> + Send + 'static,
{
    Ok(channel::spawn(config, transport).await?)
}
