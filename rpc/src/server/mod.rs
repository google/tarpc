// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a server that concurrently handles many connections sending multiplexed requests.

use crate::{
    context, util::deadline_compat, util::AsDuration, util::Compact, ClientMessage,
    ClientMessageKind, PollIo, Request, Response, ServerError, Transport,
};
use fnv::FnvHashMap;
use futures::{
    channel::mpsc,
    future::{abortable, AbortHandle},
    prelude::*,
    ready,
    stream::Fuse,
    task::{Context, Poll},
    try_ready,
};
use humantime::format_rfc3339;
use log::{debug, error, info, trace, warn};
use pin_utils::{unsafe_pinned, unsafe_unpinned};
use std::{
    error::Error as StdError,
    fmt,
    hash::Hash,
    io,
    marker::PhantomData,
    pin::Pin,
    time::{Instant, SystemTime},
};
use tokio_timer::timeout;
use trace::{self, TraceId};

/// Provides tools for limiting server connections.
pub mod filter;

/// Manages clients, serving multiplexed requests over each connection.
#[derive(Debug)]
pub struct Server<Req, Resp> {
    config: Config,
    ghost: PhantomData<(Req, Resp)>,
}

impl<Req, Resp> Default for Server<Req, Resp> {
    fn default() -> Self {
        new(Config::default())
    }
}

/// Settings that control the behavior of the server.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct Config {
    /// The maximum number of requests that can be in flight for each client. When a client is at
    /// the in-flight request limit, existing requests are fulfilled and new requests are rejected.
    /// Rejected requests are sent a response error.
    pub max_in_flight_requests_per_connection: usize,
    /// The number of responses per client that can be buffered server-side before being sent.
    /// `pending_response_buffer` controls the buffer size of the channel that a server's
    /// response tasks use to send responses to the client handler task.
    pub pending_response_buffer: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_in_flight_requests_per_connection: 1_000,
            pending_response_buffer: 100,
        }
    }
}

/// Returns a new server with configuration specified `config`.
pub fn new<Req, Resp>(config: Config) -> Server<Req, Resp> {
    Server {
        config,
        ghost: PhantomData,
    }
}

impl<Req, Resp> Server<Req, Resp> {
    /// Returns the config for this server.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Returns a stream of the incoming connections to the server.
    pub fn incoming<S, T>(
        self,
        listener: S,
    ) -> impl Stream<Item = io::Result<BaseChannel<Req, Resp, T>>>
    where
        Req: Send,
        Resp: Send,
        S: Stream<Item = io::Result<T>>,
        T: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send,
    {
        listener.map(move |r| match r {
            Ok(t) => Ok(BaseChannel {
                transport: t.fuse(),
                config: self.config.clone(),
                ghost: PhantomData,
            }),
            Err(e) => Err(e),
        })
    }
}

/// The future driving the server.
#[derive(Debug)]
pub struct Running<S, F> {
    incoming: S,
    request_handler: F,
}

impl<S, F> Running<S, F> {
    unsafe_pinned!(incoming: S);
    unsafe_unpinned!(request_handler: F);
}

impl<S, C, F, Fut> Future for Running<S, F>
where
    S: Sized + Stream<Item = io::Result<C>>,
    C: Channel + Send + 'static,
    F: FnOnce(context::Context, C::Req) -> Fut + Send + 'static + Clone,
    Fut: Future<Output = io::Result<C::Resp>> + Send + 'static,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        while let Some(channel) = ready!(self.as_mut().incoming().poll_next(cx)) {
            match channel {
                Ok(channel) => {
                    if let Err(e) =
                        crate::spawn(channel.respond_with(self.as_mut().request_handler().clone()))
                    {
                        warn!("Failed to spawn connection handler: {:?}", e);
                    }
                }
                Err(e) => {
                    warn!("Incoming connection error: {}", e);
                }
            }
        }
        info!("Server shutting down.");
        Poll::Ready(())
    }
}

/// A utility trait enabling a stream to fluently chain a request handler.
pub trait Handler<C>
where
    Self: Sized + Stream<Item = io::Result<C>>,
    C: Channel,
{
    /// Enforces limits for maximum connections.
    fn limit_connections<K, KF>(
        self,
        config: filter::Config,
        keymaker: KF,
    ) -> filter::ConnectionFilter<Self, C::Req, C::Resp, K, KF>
    where
        K: fmt::Display + Eq + Hash + Clone + Unpin,
        KF: Fn(&C::T) -> K,
    {
        filter::ConnectionFilter::new(self, config, keymaker)
    }

    /// Responds to all requests with `request_handler`.
    fn respond_with<F>(self, request_handler: F) -> Running<Self, F> {
        Running {
            incoming: self,
            request_handler,
        }
    }
}

impl<S, C> Handler<C> for S
where
    S: Sized + Stream<Item = io::Result<C>>,
    C: Channel,
{
}

/// Responds to all requests with `request_handler`.
/// The server end of an open connection with a client.
#[derive(Debug)]
pub struct BaseChannel<Req, Resp, T> {
    /// Writes responses to the wire and reads requests off the wire.
    transport: Fuse<T>,
    /// BaseChannel limits to prevent unlimited resource usage.
    config: Config,
    /// Types the request and response.
    ghost: PhantomData<(Req, Resp)>,
}

impl<Req, Resp, T> BaseChannel<Req, Resp, T> {
    unsafe_pinned!(transport: Fuse<T>);
}

impl<Req, Resp, T> BaseChannel<Req, Resp, T>
where
    T: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send,
{
    /// Creates a new channel by wrapping a transport.
    pub fn new(transport: T, config: Config) -> Self {
        BaseChannel {
            transport: transport.fuse(),
            config,
            ghost: PhantomData,
        }
    }

    /// Returns a reference to the inner transport.
    pub fn get_ref(&self) -> &T {
        self.transport.get_ref()
    }
}

/// Responds to all requests with `request_handler`.
/// The server end of an open connection with a client.
pub trait Channel
where
    Self: Sized,
{
    /// Type of request item.
    type Req: Send + 'static;

    /// Type of response sink item.
    type Resp: Send + 'static;

    /// The type of the inner transport.
    type T: Transport<Item = ClientMessage<Self::Req>, SinkItem = Response<Self::Resp>>
        + Sink<Response<Self::Resp>, SinkError = io::Error>
        + Send
        + 'static;

    /// Returns the server configuration.
    fn config(&self) -> &Config;

    /// Returns a reference to the inner transport.
    fn get_ref(&self) -> &Self::T;

    /// Returns a pinned mutable reference to the inner transport.
    fn transport<'a>(self: Pin<&'a mut Self>) -> Pin<&'a mut Self::T>;

    /// Respond to requests coming over the channel with `f`. Returns a future that drives the
    /// responses and resolves when the connection is closed.
    fn respond_with<F, Fut>(self, f: F) -> ClientHandler<Self, F>
    where
        F: FnOnce(context::Context, Self::Req) -> Fut + Send + 'static + Clone,
        Fut: Future<Output = io::Result<Self::Resp>> + Send + 'static,
    {
        let (responses_tx, responses) = mpsc::channel(self.config().pending_response_buffer);
        let responses = responses.fuse();

        ClientHandler {
            channel: self,
            f,
            pending_responses: responses,
            responses_tx,
            in_flight_requests: FnvHashMap::default(),
        }
    }
}

impl<Req, Resp, T> Channel for BaseChannel<Req, Resp, T>
where
    T: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send + 'static,
    Req: Send + 'static,
    Resp: Send + 'static,
{
    type Req = Req;
    type Resp = Resp;
    type T = T;

    /// Returns a mutable reference to the inner transport.
    fn transport<'a>(self: Pin<&'a mut Self>) -> Pin<&'a mut T> {
        unsafe { Self::transport(self).map_unchecked_mut(Fuse::get_mut) }
    }

    /// Returns the server configuration.
    fn config(&self) -> &Config {
        &self.config
    }

    fn get_ref(&self) -> &T {
        self.transport.get_ref()
    }
}

/// A running handler serving all requests for a single client.
#[derive(Debug)]
pub struct ClientHandler<C, F>
where
    C: Channel,
{
    channel: C,
    /// Responses waiting to be written to the wire.
    pending_responses: Fuse<mpsc::Receiver<(context::Context, Response<C::Resp>)>>,
    /// Handed out to request handlers to fan in responses.
    responses_tx: mpsc::Sender<(context::Context, Response<C::Resp>)>,
    /// Number of requests currently being responded to.
    in_flight_requests: FnvHashMap<u64, AbortHandle>,
    /// Request handler.
    f: F,
}

impl<C, F> ClientHandler<C, F>
where
    C: Channel,
{
    unsafe_pinned!(channel: C);
    unsafe_pinned!(in_flight_requests: FnvHashMap<u64, AbortHandle>);
    unsafe_pinned!(pending_responses: Fuse<mpsc::Receiver<(context::Context, Response<C::Resp>)>>);
    unsafe_pinned!(responses_tx: mpsc::Sender<(context::Context, Response<C::Resp>)>);
    // For this to be safe, field f must be private, and code in this module must never
    // construct PinMut<F>.
    unsafe_unpinned!(f: F);
}

impl<C, F, Fut> ClientHandler<C, F>
where
    C: Channel,
    F: FnOnce(context::Context, C::Req) -> Fut + Send + 'static + Clone,
    Fut: Future<Output = io::Result<C::Resp>> + Send + 'static,
{
    /// If at max in-flight requests, check that there's room to immediately write a throttled
    /// response.
    fn poll_ready_if_throttling(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        if self.in_flight_requests.len()
            >= self.channel.config().max_in_flight_requests_per_connection
        {
            while let Poll::Pending = self.as_mut().channel().transport().poll_ready(cx)? {
                info!(
                    "In-flight requests at max ({}), and transport is not ready.",
                    self.as_mut().in_flight_requests().len(),
                );
                try_ready!(self.as_mut().channel().transport().poll_flush(cx));
            }
        }
        Poll::Ready(Ok(()))
    }

    fn pump_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> PollIo<()> {
        ready!(self.as_mut().poll_ready_if_throttling(cx)?);

        Poll::Ready(
            match ready!(self.as_mut().channel().transport().poll_next(cx)?) {
                Some(message) => {
                    match message.message {
                        ClientMessageKind::Request(request) => {
                            self.handle_request(message.trace_context, request)?;
                        }
                        ClientMessageKind::Cancel { request_id } => {
                            self.cancel_request(&message.trace_context, request_id);
                        }
                    }
                    Some(Ok(()))
                }
                None => None,
            },
        )
    }

    fn pump_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        read_half_closed: bool,
    ) -> PollIo<()> {
        match self.as_mut().poll_next_response(cx)? {
            Poll::Ready(Some((_, response))) => {
                self.as_mut().channel().transport().start_send(response)?;
                Poll::Ready(Some(Ok(())))
            }
            Poll::Ready(None) => {
                // Shutdown can't be done before we finish pumping out remaining responses.
                ready!(self.as_mut().channel().transport().poll_flush(cx)?);
                Poll::Ready(None)
            }
            Poll::Pending => {
                // No more requests to process, so flush any requests buffered in the transport.
                ready!(self.as_mut().channel().transport().poll_flush(cx)?);

                // Being here means there are no staged requests and all written responses are
                // fully flushed. So, if the read half is closed and there are no in-flight
                // requests, then we can close the write half.
                if read_half_closed && self.as_mut().in_flight_requests().is_empty() {
                    Poll::Ready(None)
                } else {
                    Poll::Pending
                }
            }
        }
    }

    fn poll_next_response(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> PollIo<(context::Context, Response<C::Resp>)> {
        // Ensure there's room to write a response.
        while let Poll::Pending = self.as_mut().channel().transport().poll_ready(cx)? {
            ready!(self.as_mut().channel().transport().poll_flush(cx)?);
        }

        match ready!(self.as_mut().pending_responses().poll_next(cx)) {
            Some((ctx, response)) => {
                if self
                    .as_mut()
                    .in_flight_requests()
                    .remove(&response.request_id)
                    .is_some()
                {
                    self.as_mut().in_flight_requests().compact(0.1);
                }
                trace!(
                    "[{}] Staging response. In-flight requests = {}.",
                    ctx.trace_id(),
                    self.as_mut().in_flight_requests().len(),
                );
                Poll::Ready(Some(Ok((ctx, response))))
            }
            None => {
                // This branch likely won't happen, since the ClientHandler is holding a Sender.
                Poll::Ready(None)
            }
        }
    }

    fn handle_request(
        mut self: Pin<&mut Self>,
        trace_context: trace::Context,
        request: Request<C::Req>,
    ) -> io::Result<()> {
        let request_id = request.id;
        let ctx = context::Context {
            deadline: request.deadline,
            trace_context,
        };
        let request = request.message;

        if self.as_mut().in_flight_requests().len()
            >= self
                .as_mut()
                .channel()
                .config()
                .max_in_flight_requests_per_connection
        {
            debug!(
                "[{}] Client has reached in-flight request limit ({}/{}).",
                ctx.trace_id(),
                self.as_mut().in_flight_requests().len(),
                self.as_mut()
                    .channel()
                    .config()
                    .max_in_flight_requests_per_connection
            );

            self.as_mut().channel().transport().start_send(Response {
                request_id,
                message: Err(ServerError {
                    kind: io::ErrorKind::WouldBlock,
                    detail: Some("Server throttled the request.".into()),
                }),
            })?;
            return Ok(());
        }

        let deadline = ctx.deadline;
        let timeout = deadline.as_duration();
        trace!(
            "[{}] Received request with deadline {} (timeout {:?}).",
            ctx.trace_id(),
            format_rfc3339(deadline),
            timeout,
        );
        let mut response_tx = self.as_mut().responses_tx().clone();

        let trace_id = *ctx.trace_id();
        let response = self.as_mut().f().clone()(ctx, request);
        let response = deadline_compat::Deadline::new(response, Instant::now() + timeout).then(
            async move |result| {
                let response = Response {
                    request_id,
                    message: match result {
                        Ok(message) => Ok(message),
                        Err(e) => Err(make_server_error(e, trace_id, deadline)),
                    },
                };
                trace!("[{}] Sending response.", trace_id);
                response_tx
                    .send((ctx, response))
                    .unwrap_or_else(|_| ())
                    .await;
            },
        );
        let (abortable_response, abort_handle) = abortable(response);
        crate::spawn(abortable_response.map(|_| ())).map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "Could not spawn response task. Is shutdown: {}",
                    e.is_shutdown()
                ),
            )
        })?;
        self.as_mut()
            .in_flight_requests()
            .insert(request_id, abort_handle);
        Ok(())
    }

    fn cancel_request(mut self: Pin<&mut Self>, trace_context: &trace::Context, request_id: u64) {
        // It's possible the request was already completed, so it's fine
        // if this is None.
        if let Some(cancel_handle) = self.as_mut().in_flight_requests().remove(&request_id) {
            self.as_mut().in_flight_requests().compact(0.1);

            cancel_handle.abort();
            let remaining = self.as_mut().in_flight_requests().len();
            trace!(
                "[{}] Request canceled. In-flight requests = {}",
                trace_context.trace_id,
                remaining,
            );
        } else {
            trace!(
                "[{}] Received cancellation, but response handler \
                 is already complete.",
                trace_context.trace_id,
            );
        }
    }
}

impl<C, F, Fut> Future for ClientHandler<C, F>
where
    C: Channel,
    F: FnOnce(context::Context, C::Req) -> Fut + Send + 'static + Clone,
    Fut: Future<Output = io::Result<C::Resp>> + Send + 'static,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        move || -> Poll<io::Result<()>> {
            loop {
                let read = self.as_mut().pump_read(cx)?;
                match (
                    read,
                    self.as_mut().pump_write(cx, read == Poll::Ready(None))?,
                ) {
                    (Poll::Ready(None), Poll::Ready(None)) => {
                        return Poll::Ready(Ok(()));
                    }
                    (Poll::Ready(Some(())), _) | (_, Poll::Ready(Some(()))) => {}
                    _ => {
                        return Poll::Pending;
                    }
                }
            }
        }()
        .map(|r| r.unwrap_or_else(|e| info!("ClientHandler errored out: {}", e)))
    }
}

fn make_server_error(
    e: timeout::Error<io::Error>,
    trace_id: TraceId,
    deadline: SystemTime,
) -> ServerError {
    if e.is_elapsed() {
        debug!(
            "[{}] Response did not complete before deadline of {}s.",
            trace_id,
            format_rfc3339(deadline)
        );
        // No point in responding, since the client will have dropped the request.
        ServerError {
            kind: io::ErrorKind::TimedOut,
            detail: Some(format!(
                "Response did not complete before deadline of {}s.",
                format_rfc3339(deadline)
            )),
        }
    } else if e.is_timer() {
        error!(
            "[{}] Response failed because of an issue with a timer: {}",
            trace_id, e
        );

        ServerError {
            kind: io::ErrorKind::Other,
            detail: Some(format!("{}", e)),
        }
    } else if e.is_inner() {
        let e = e.into_inner().unwrap();
        ServerError {
            kind: e.kind(),
            detail: Some(e.description().into()),
        }
    } else {
        error!("[{}] Unexpected response failure: {}", trace_id, e);

        ServerError {
            kind: io::ErrorKind::Other,
            detail: Some(format!("Server unexpectedly failed to respond: {}", e)),
        }
    }
}
