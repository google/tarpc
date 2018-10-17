// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a server that concurrently handles many connections sending multiplexed requests.

use crate::{
    context::Context, util::deadline_compat, util::AsDuration, util::Compact, ClientMessage,
    ClientMessageKind, Request, Response, ServerError, Transport,
};
use fnv::FnvHashMap;
use futures::{
    channel::mpsc,
    future::{abortable, AbortHandle},
    prelude::*,
    ready,
    stream::Fuse,
    task::{LocalWaker, Poll},
    try_ready,
};
use humantime::format_rfc3339;
use log::{debug, error, info, trace, warn};
use pin_utils::{unsafe_pinned, unsafe_unpinned};
use std::{
    error::Error as StdError,
    io,
    marker::PhantomData,
    net::SocketAddr,
    pin::Pin,
    time::{Instant, SystemTime},
};
use tokio_timer::timeout;
use trace::{self, TraceId};

mod filter;

/// Manages clients, serving multiplexed requests over each connection.
#[derive(Debug)]
pub struct Server<Req, Resp> {
    config: Config,
    ghost: PhantomData<(Req, Resp)>,
}

/// Settings that control the behavior of the server.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct Config {
    /// The maximum number of clients that can be connected to the server at once. When at the
    /// limit, existing connections are honored and new connections are rejected.
    pub max_connections: usize,
    /// The maximum number of clients per IP address that can be connected to the server at once.
    /// When an IP is at the limit, existing connections are honored and new connections on that IP
    /// address are rejected.
    pub max_connections_per_ip: usize,
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
            max_connections: 1_000_000,
            max_connections_per_ip: 1_000,
            max_in_flight_requests_per_connection: 1_000,
            pending_response_buffer: 100,
        }
    }
}

impl<Req, Resp> Server<Req, Resp> {
    /// Returns a new server with configuration specified `config`.
    pub fn new(config: Config) -> Self {
        Server {
            config,
            ghost: PhantomData,
        }
    }

    /// Returns the config for this server.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Returns a stream of the incoming connections to the server.
    pub fn incoming<S, T>(
        self,
        listener: S,
    ) -> impl Stream<Item = io::Result<Channel<Req, Resp, T>>>
    where
        Req: Send,
        Resp: Send,
        S: Stream<Item = io::Result<T>>,
        T: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send,
    {
        self::filter::ConnectionFilter::filter(listener, self.config.clone())
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

impl<S, T, Req, Resp, F, Fut> Future for Running<S, F>
where
    S: Sized + Stream<Item = io::Result<Channel<Req, Resp, T>>>,
    Req: Send + 'static,
    Resp: Send + 'static,
    T: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send + 'static,
    F: FnMut(Context, Req) -> Fut + Send + 'static + Clone,
    Fut: Future<Output = io::Result<Resp>> + Send + 'static,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &LocalWaker) -> Poll<()> {
        while let Some(channel) = ready!(self.incoming().poll_next(cx)) {
            match channel {
                Ok(channel) => {
                    let peer = channel.client_addr;
                    if let Err(e) = crate::spawn(channel.respond_with(self.request_handler().clone()))
                    {
                        warn!("[{}] Failed to spawn connection handler: {:?}", peer, e);
                    }
                }
                Err(e) => {
                    warn!("Incoming connection error: {}", e);
                }
            }
        }
        info!("Server shutting down.");
        return Poll::Ready(());
    }
}

/// A utility trait enabling a stream to fluently chain a request handler.
pub trait Handler<T, Req, Resp>
where
    Self: Sized + Stream<Item = io::Result<Channel<Req, Resp, T>>>,
    Req: Send,
    Resp: Send,
    T: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send,
{
    /// Responds to all requests with `request_handler`.
    fn respond_with<F, Fut>(self, request_handler: F) -> Running<Self, F>
    where
        F: FnMut(Context, Req) -> Fut + Send + 'static + Clone,
        Fut: Future<Output = io::Result<Resp>> + Send + 'static,
    {
        Running {
            incoming: self,
            request_handler,
        }
    }
}

impl<T, Req, Resp, S> Handler<T, Req, Resp> for S
where
    S: Sized + Stream<Item = io::Result<Channel<Req, Resp, T>>>,
    Req: Send,
    Resp: Send,
    T: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send,
{}

/// Responds to all requests with `request_handler`.
/// The server end of an open connection with a client.
#[derive(Debug)]
pub struct Channel<Req, Resp, T> {
    /// Writes responses to the wire and reads requests off the wire.
    transport: Fuse<T>,
    /// Signals the connection is closed when `Channel` is dropped.
    closed_connections: mpsc::UnboundedSender<SocketAddr>,
    /// Channel limits to prevent unlimited resource usage.
    config: Config,
    /// The address of the server connected to.
    client_addr: SocketAddr,
    /// Types the request and response.
    ghost: PhantomData<(Req, Resp)>,
}

impl<Req, Resp, T> Drop for Channel<Req, Resp, T> {
    fn drop(&mut self) {
        trace!("[{}] Closing channel.", self.client_addr);

        // Even in a bounded channel, each connection would have a guaranteed slot, so using
        // an unbounded sender is actually no different. And, the bound is on the maximum number
        // of open connections.
        if self
            .closed_connections
            .unbounded_send(self.client_addr)
            .is_err()
        {
            warn!(
                "[{}] Failed to send closed connection message.",
                self.client_addr
            );
        }
    }
}

impl<Req, Resp, T> Channel<Req, Resp, T> {
    unsafe_pinned!(transport: Fuse<T>);
}

impl<Req, Resp, T> Channel<Req, Resp, T>
where
    T: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send,
    Req: Send,
    Resp: Send,
{
    pub(crate) fn start_send(self: &mut Pin<&mut Self>, response: Response<Resp>) -> io::Result<()> {
        self.transport().start_send(response)
    }

    pub(crate) fn poll_ready(
        self: &mut Pin<&mut Self>,
        cx: &LocalWaker,
    ) -> Poll<io::Result<()>> {
        self.transport().poll_ready(cx)
    }

    pub(crate) fn poll_flush(
        self: &mut Pin<&mut Self>,
        cx: &LocalWaker,
    ) -> Poll<io::Result<()>> {
        self.transport().poll_flush(cx)
    }

    pub(crate) fn poll_next(
        self: &mut Pin<&mut Self>,
        cx: &LocalWaker,
    ) -> Poll<Option<io::Result<ClientMessage<Req>>>> {
        self.transport().poll_next(cx)
    }

    /// Returns the address of the client connected to the channel.
    pub fn client_addr(&self) -> &SocketAddr {
        &self.client_addr
    }

    /// Respond to requests coming over the channel with `f`. Returns a future that drives the
    /// responses and resolves when the connection is closed.
    pub fn respond_with<F, Fut>(self, f: F) -> impl Future<Output = ()>
    where
        F: FnMut(Context, Req) -> Fut + Send + 'static,
        Fut: Future<Output = io::Result<Resp>> + Send + 'static,
        Req: 'static,
        Resp: 'static,
    {
        let (responses_tx, responses) = mpsc::channel(self.config.pending_response_buffer);
        let responses = responses.fuse();
        let peer = self.client_addr;

        ClientHandler {
            channel: self,
            f,
            pending_responses: responses,
            responses_tx,
            in_flight_requests: FnvHashMap::default(),
        }.unwrap_or_else(move |e| {
            info!("[{}] ClientHandler errored out: {}", peer, e);
        })
    }
}

#[derive(Debug)]
struct ClientHandler<Req, Resp, T, F> {
    channel: Channel<Req, Resp, T>,
    /// Responses waiting to be written to the wire.
    pending_responses: Fuse<mpsc::Receiver<(Context, Response<Resp>)>>,
    /// Handed out to request handlers to fan in responses.
    responses_tx: mpsc::Sender<(Context, Response<Resp>)>,
    /// Number of requests currently being responded to.
    in_flight_requests: FnvHashMap<u64, AbortHandle>,
    /// Request handler.
    f: F,
}

impl<Req, Resp, T, F> ClientHandler<Req, Resp, T, F> {
    unsafe_pinned!(channel: Channel<Req, Resp, T>);
    unsafe_pinned!(in_flight_requests: FnvHashMap<u64, AbortHandle>);
    unsafe_pinned!(pending_responses: Fuse<mpsc::Receiver<(Context, Response<Resp>)>>);
    unsafe_pinned!(responses_tx: mpsc::Sender<(Context, Response<Resp>)>);
    // For this to be safe, field f must be private, and code in this module must never
    // construct PinMut<F>.
    unsafe_unpinned!(f: F);
}

impl<Req, Resp, T, F, Fut> ClientHandler<Req, Resp, T, F>
where
    Req: Send + 'static,
    Resp: Send + 'static,
    T: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send,
    F: FnMut(Context, Req) -> Fut + Send + 'static,
    Fut: Future<Output = io::Result<Resp>> + Send + 'static,
{
    /// If at max in-flight requests, check that there's room to immediately write a throttled
    /// response.
    fn poll_ready_if_throttling(
        self: &mut Pin<&mut Self>,
        cx: &LocalWaker,
    ) -> Poll<io::Result<()>> {
        if self.in_flight_requests.len()
            >= self.channel.config.max_in_flight_requests_per_connection
        {
            let peer = self.channel().client_addr;

            while let Poll::Pending = self.channel().poll_ready(cx)? {
                info!(
                    "[{}] In-flight requests at max ({}), and transport is not ready.",
                    peer,
                    self.in_flight_requests().len(),
                );
                try_ready!(self.channel().poll_flush(cx));
            }
        }
        Poll::Ready(Ok(()))
    }

    fn pump_read(self: &mut Pin<&mut Self>, cx: &LocalWaker) -> Poll<Option<io::Result<()>>> {
        ready!(self.poll_ready_if_throttling(cx)?);

        Poll::Ready(match ready!(self.channel().poll_next(cx)?) {
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
            None => {
                trace!("[{}] Read half closed", self.channel.client_addr);
                None
            }
        })
    }

    fn pump_write(
        self: &mut Pin<&mut Self>,
        cx: &LocalWaker,
        read_half_closed: bool,
    ) -> Poll<Option<io::Result<()>>> {
        match self.poll_next_response(cx)? {
            Poll::Ready(Some((_, response))) => {
                self.channel().start_send(response)?;
                Poll::Ready(Some(Ok(())))
            }
            Poll::Ready(None) => {
                // Shutdown can't be done before we finish pumping out remaining responses.
                ready!(self.channel().poll_flush(cx)?);
                Poll::Ready(None)
            }
            Poll::Pending => {
                // No more requests to process, so flush any requests buffered in the transport.
                ready!(self.channel().poll_flush(cx)?);

                // Being here means there are no staged requests and all written responses are
                // fully flushed. So, if the read half is closed and there are no in-flight
                // requests, then we can close the write half.
                if read_half_closed && self.in_flight_requests().is_empty() {
                    Poll::Ready(None)
                } else {
                    Poll::Pending
                }
            }
        }
    }

    fn poll_next_response(
        self: &mut Pin<&mut Self>,
        cx: &LocalWaker,
    ) -> Poll<Option<io::Result<(Context, Response<Resp>)>>> {
        // Ensure there's room to write a response.
        while let Poll::Pending = self.channel().poll_ready(cx)? {
            ready!(self.channel().poll_flush(cx)?);
        }

        let peer = self.channel().client_addr;

        match ready!(self.pending_responses().poll_next(cx)) {
            Some((ctx, response)) => {
                if let Some(_) = self.in_flight_requests().remove(&response.request_id) {
                    self.in_flight_requests().compact(0.1);
                }
                trace!(
                    "[{}/{}] Staging response. In-flight requests = {}.",
                    ctx.trace_id(),
                    peer,
                    self.in_flight_requests().len(),
                );
                return Poll::Ready(Some(Ok((ctx, response))));
            }
            None => {
                // This branch likely won't happen, since the ClientHandler is holding a Sender.
                trace!("[{}] No new responses.", peer);
                Poll::Ready(None)
            }
        }
    }

    fn handle_request(
        self: &mut Pin<&mut Self>,
        trace_context: trace::Context,
        request: Request<Req>,
    ) -> io::Result<()> {
        let request_id = request.id;
        let peer = self.channel().client_addr;
        let ctx = Context {
            deadline: request.deadline,
            trace_context,
        };
        let request = request.message;

        if self.in_flight_requests().len()
            >= self.channel().config.max_in_flight_requests_per_connection
        {
            debug!(
                "[{}/{}] Client has reached in-flight request limit ({}/{}).",
                ctx.trace_id(),
                peer,
                self.in_flight_requests().len(),
                self.channel().config.max_in_flight_requests_per_connection
            );

            self.channel().start_send(Response {
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
            "[{}/{}] Received request with deadline {} (timeout {:?}).",
            ctx.trace_id(),
            peer,
            format_rfc3339(deadline),
            timeout,
        );
        let mut response_tx = self.responses_tx().clone();

        let trace_id = *ctx.trace_id();
        let response = self.f()(ctx.clone(), request);
        let response = deadline_compat::Deadline::new(response, Instant::now() + timeout).then(
            async move |result| {
                let response = Response {
                    request_id,
                    message: match result {
                        Ok(message) => Ok(message),
                        Err(e) => Err(make_server_error(e, trace_id, peer, deadline)),
                    },
                };
                trace!("[{}/{}] Sending response.", trace_id, peer);
                await!(response_tx.send((ctx, response)).unwrap_or_else(|_| ()));
            },
        );
        let (abortable_response, abort_handle) = abortable(response);
        crate::spawn(abortable_response.map(|_| ()))
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "Could not spawn response task. Is shutdown: {}",
                        e.is_shutdown()
                    ),
                )
            })?;
        self.in_flight_requests().insert(request_id, abort_handle);
        Ok(())
    }

    fn cancel_request(self: &mut Pin<&mut Self>, trace_context: &trace::Context, request_id: u64) {
        // It's possible the request was already completed, so it's fine
        // if this is None.
        if let Some(cancel_handle) = self.in_flight_requests().remove(&request_id) {
            self.in_flight_requests().compact(0.1);

            cancel_handle.abort();
            let remaining = self.in_flight_requests().len();
            trace!(
                "[{}/{}] Request canceled. In-flight requests = {}",
                trace_context.trace_id,
                self.channel.client_addr,
                remaining,
            );
        } else {
            trace!(
                "[{}/{}] Received cancellation, but response handler \
                 is already complete.",
                trace_context.trace_id,
                self.channel.client_addr
            );
        }
    }
}

impl<Req, Resp, T, F, Fut> Future for ClientHandler<Req, Resp, T, F>
where
    Req: Send + 'static,
    Resp: Send + 'static,
    T: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send,
    F: FnMut(Context, Req) -> Fut + Send + 'static,
    Fut: Future<Output = io::Result<Resp>> + Send + 'static,
{
    type Output = io::Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &LocalWaker) -> Poll<io::Result<()>> {
        trace!("[{}] ClientHandler::poll", self.channel.client_addr);
        loop {
            let read = self.pump_read(cx)?;
            match (read, self.pump_write(cx, read == Poll::Ready(None))?) {
                (Poll::Ready(None), Poll::Ready(None)) => {
                    info!("[{}] Client disconnected.", self.channel.client_addr);
                    return Poll::Ready(Ok(()));
                }
                (read @ Poll::Ready(Some(())), write) | (read, write @ Poll::Ready(Some(()))) => {
                    trace!(
                        "[{}] read: {:?}, write: {:?}.",
                        self.channel.client_addr,
                        read,
                        write
                    )
                }
                (read, write) => {
                    trace!(
                        "[{}] read: {:?}, write: {:?} (not ready).",
                        self.channel.client_addr,
                        read,
                        write,
                    );
                    return Poll::Pending;
                }
            }
        }
    }
}

fn make_server_error(
    e: timeout::Error<io::Error>,
    trace_id: TraceId,
    peer: SocketAddr,
    deadline: SystemTime,
) -> ServerError {
    if e.is_elapsed() {
        debug!(
            "[{}/{}] Response did not complete before deadline of {}s.",
            trace_id,
            peer,
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
            "[{}/{}] Response failed because of an issue with a timer: {}",
            trace_id, peer, e
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
        error!("[{}/{}] Unexpected response failure: {}", trace_id, peer, e);

        ServerError {
            kind: io::ErrorKind::Other,
            detail: Some(format!("Server unexpectedly failed to respond: {}", e)),
        }
    }
}
