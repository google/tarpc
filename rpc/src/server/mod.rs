// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a server that concurrently handles many connections sending multiplexed requests.

use crate::{
    context, util::deadline_compat, util::AsDuration, util::Compact, ClientMessage, PollIo,
    Request, Response, ServerError, Transport,
};
use fnv::FnvHashMap;
use futures::{
    channel::mpsc,
    future::{AbortHandle, AbortRegistration, Abortable},
    prelude::*,
    ready,
    stream::Fuse,
    task::{Context, Poll},
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

mod filter;
#[cfg(test)]
mod testing;
mod throttle;

pub use self::{
    filter::ChannelFilter,
    throttle::{Throttler, ThrottlerStream},
};

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
    /// The number of responses per client that can be buffered server-side before being sent.
    /// `pending_response_buffer` controls the buffer size of the channel that a server's
    /// response tasks use to send responses to the client handler task.
    pub pending_response_buffer: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            pending_response_buffer: 100,
        }
    }
}

impl Config {
    /// Returns a channel backed by `transport` and configured with `self`.
    pub fn channel<Req, Resp, T>(self, transport: T) -> BaseChannel<Req, Resp, T>
    where
        T: Transport<Response<Resp>, ClientMessage<Req>> + Send,
    {
        BaseChannel::new(self, transport)
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

    /// Returns a stream of server channels.
    pub fn incoming<S, T>(self, listener: S) -> impl Stream<Item = BaseChannel<Req, Resp, T>>
    where
        Req: Send,
        Resp: Send,
        S: Stream<Item = T>,
        T: Transport<Response<Resp>, ClientMessage<Req>> + Send,
    {
        listener.map(move |t| BaseChannel::new(self.config.clone(), t))
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
    S: Sized + Stream<Item = C>,
    C: Channel + Send + 'static,
    F: FnOnce(context::Context, C::Req) -> Fut + Send + 'static + Clone,
    Fut: Future<Output = io::Result<C::Resp>> + Send + 'static,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        while let Some(channel) = ready!(self.as_mut().incoming().poll_next(cx)) {
            if let Err(e) =
                crate::spawn(channel.respond_with(self.as_mut().request_handler().clone()))
            {
                warn!("Failed to spawn channel handler: {:?}", e);
            }
        }
        info!("Server shutting down.");
        Poll::Ready(())
    }
}

/// A utility trait enabling a stream to fluently chain a request handler.
pub trait Handler<C>
where
    Self: Sized + Stream<Item = C>,
    C: Channel,
{
    /// Enforces channel per-key limits.
    fn max_channels_per_key<K, KF>(self, n: u32, keymaker: KF) -> filter::ChannelFilter<Self, K, KF>
    where
        K: fmt::Display + Eq + Hash + Clone + Unpin,
        KF: Fn(&C) -> K,
    {
        ChannelFilter::new(self, n, keymaker)
    }

    /// Caps the number of concurrent requests per channel.
    fn max_concurrent_requests_per_channel(self, n: usize) -> ThrottlerStream<Self> {
        ThrottlerStream::new(self, n)
    }

    /// Responds to all requests with `request_handler`.
    fn respond_with<F, Fut>(self, request_handler: F) -> Running<Self, F>
    where
        F: FnOnce(context::Context, C::Req) -> Fut + Send + 'static + Clone,
        Fut: Future<Output = io::Result<C::Resp>> + Send + 'static,
    {
        Running {
            incoming: self,
            request_handler,
        }
    }
}

impl<S, C> Handler<C> for S
where
    S: Sized + Stream<Item = C>,
    C: Channel,
{
}

/// BaseChannel lifts a Transport to a Channel by tracking in-flight requests.
#[derive(Debug)]
pub struct BaseChannel<Req, Resp, T> {
    config: Config,
    /// Writes responses to the wire and reads requests off the wire.
    transport: Fuse<T>,
    /// Number of requests currently being responded to.
    in_flight_requests: FnvHashMap<u64, AbortHandle>,
    /// Types the request and response.
    ghost: PhantomData<(Req, Resp)>,
}

impl<Req, Resp, T> BaseChannel<Req, Resp, T> {
    unsafe_unpinned!(in_flight_requests: FnvHashMap<u64, AbortHandle>);
}

impl<Req, Resp, T> BaseChannel<Req, Resp, T>
where
    T: Transport<Response<Resp>, ClientMessage<Req>> + Send,
{
    /// Creates a new channel backed by `transport` and configured with `config`.
    pub fn new(config: Config, transport: T) -> Self {
        BaseChannel {
            config,
            transport: transport.fuse(),
            in_flight_requests: FnvHashMap::default(),
            ghost: PhantomData,
        }
    }

    /// Creates a new channel backed by `transport` and configured with the defaults.
    pub fn with_defaults(transport: T) -> Self {
        Self::new(Config::default(), transport)
    }

    /// Returns the inner transport.
    pub fn get_ref(&self) -> &T {
        self.transport.get_ref()
    }

    /// Returns the pinned inner transport.
    pub fn transport<'a>(self: Pin<&'a mut Self>) -> Pin<&'a mut T> {
        unsafe { self.map_unchecked_mut(|me| me.transport.get_mut()) }
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

/// The server end of an open connection with a client, streaming in requests from, and sinking
/// responses to, the client.
///
/// Channels are free to somewhat rely on the assumption that all in-flight requests are eventually
/// either [cancelled](Channel::cancel_request) or [responded to](Sink::start_send). Safety cannot
/// rely on this assumption, but it is best for `Channel` users to always account for all outstanding
/// requests.
pub trait Channel
where
    Self: Transport<Response<<Self as Channel>::Resp>, Request<<Self as Channel>::Req>>,
{
    /// Type of request item.
    type Req: Send + 'static;

    /// Type of response sink item.
    type Resp: Send + 'static;

    /// Configuration of the channel.
    fn config(&self) -> &Config;

    /// Returns the number of in-flight requests over this channel.
    fn in_flight_requests(self: Pin<&mut Self>) -> usize;

    /// Caps the number of concurrent requests.
    fn max_concurrent_requests(self, n: usize) -> Throttler<Self>
    where
        Self: Sized,
    {
        Throttler::new(self, n)
    }

    /// Tells the Channel that request with ID `request_id` is being handled.
    /// The request will be tracked until a response with the same ID is sent
    /// to the Channel.
    fn start_request(self: Pin<&mut Self>, request_id: u64) -> AbortRegistration;

    /// Respond to requests coming over the channel with `f`. Returns a future that drives the
    /// responses and resolves when the connection is closed.
    fn respond_with<F, Fut>(self, f: F) -> ResponseHandler<Self, F>
    where
        F: FnOnce(context::Context, Self::Req) -> Fut + Send + 'static + Clone,
        Fut: Future<Output = io::Result<Self::Resp>> + Send + 'static,
        Self: Sized,
    {
        let (responses_tx, responses) = mpsc::channel(self.config().pending_response_buffer);
        let responses = responses.fuse();

        ResponseHandler {
            channel: self,
            f,
            pending_responses: responses,
            responses_tx,
        }
    }
}

impl<Req, Resp, T> Stream for BaseChannel<Req, Resp, T>
where
    T: Transport<Response<Resp>, ClientMessage<Req>> + Send + 'static,
    Req: Send + 'static,
    Resp: Send + 'static,
{
    type Item = io::Result<Request<Req>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        loop {
            match ready!(self.as_mut().transport().poll_next(cx)?) {
                Some(message) => match message {
                    ClientMessage::Request(request) => {
                        return Poll::Ready(Some(Ok(request)));
                    }
                    ClientMessage::Cancel {
                        trace_context,
                        request_id,
                    } => {
                        self.as_mut().cancel_request(&trace_context, request_id);
                    }
                },
                None => return Poll::Ready(None),
            }
        }
    }
}

impl<Req, Resp, T> Sink<Response<Resp>> for BaseChannel<Req, Resp, T>
where
    T: Transport<Response<Resp>, ClientMessage<Req>> + Send + 'static,
    Req: Send + 'static,
    Resp: Send + 'static,
{
    type SinkError = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::SinkError>> {
        self.transport().poll_ready(cx)
    }

    fn start_send(
        mut self: Pin<&mut Self>,
        response: Response<Resp>,
    ) -> Result<(), Self::SinkError> {
        if self
            .as_mut()
            .in_flight_requests()
            .remove(&response.request_id)
            .is_some()
        {
            self.as_mut().in_flight_requests().compact(0.1);
        }

        self.transport().start_send(response)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::SinkError>> {
        self.transport().poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::SinkError>> {
        self.transport().poll_close(cx)
    }
}

impl<Req, Resp, T> AsRef<T> for BaseChannel<Req, Resp, T> {
    fn as_ref(&self) -> &T {
        self.transport.get_ref()
    }
}

impl<Req, Resp, T> Channel for BaseChannel<Req, Resp, T>
where
    T: Transport<Response<Resp>, ClientMessage<Req>> + Send + 'static,
    Req: Send + 'static,
    Resp: Send + 'static,
{
    type Req = Req;
    type Resp = Resp;

    fn config(&self) -> &Config {
        &self.config
    }

    fn in_flight_requests(mut self: Pin<&mut Self>) -> usize {
        self.as_mut().in_flight_requests().len()
    }

    fn start_request(self: Pin<&mut Self>, request_id: u64) -> AbortRegistration {
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        assert!(self
            .in_flight_requests()
            .insert(request_id, abort_handle)
            .is_none());
        abort_registration
    }
}

/// A running handler serving all requests coming over a channel.
#[derive(Debug)]
pub struct ResponseHandler<C, F>
where
    C: Channel,
{
    channel: C,
    /// Responses waiting to be written to the wire.
    pending_responses: Fuse<mpsc::Receiver<(context::Context, Response<C::Resp>)>>,
    /// Handed out to request handlers to fan in responses.
    responses_tx: mpsc::Sender<(context::Context, Response<C::Resp>)>,
    /// Request handler.
    f: F,
}

impl<C, F> ResponseHandler<C, F>
where
    C: Channel,
{
    unsafe_pinned!(channel: C);
    unsafe_pinned!(pending_responses: Fuse<mpsc::Receiver<(context::Context, Response<C::Resp>)>>);
    unsafe_pinned!(responses_tx: mpsc::Sender<(context::Context, Response<C::Resp>)>);
    // For this to be safe, field f must be private, and code in this module must never
    // construct PinMut<F>.
    unsafe_unpinned!(f: F);
}

impl<C, F, Fut> ResponseHandler<C, F>
where
    C: Channel,
    F: FnOnce(context::Context, C::Req) -> Fut + Send + 'static + Clone,
    Fut: Future<Output = io::Result<C::Resp>> + Send + 'static,
{
    fn pump_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> PollIo<()> {
        match ready!(self.as_mut().channel().poll_next(cx)?) {
            Some(request) => {
                self.handle_request(request)?;
                Poll::Ready(Some(Ok(())))
            }
            None => Poll::Ready(None),
        }
    }

    fn pump_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        read_half_closed: bool,
    ) -> PollIo<()> {
        match self.as_mut().poll_next_response(cx)? {
            Poll::Ready(Some((ctx, response))) => {
                trace!(
                    "[{}] Staging response. In-flight requests = {}.",
                    ctx.trace_id(),
                    self.as_mut().channel().in_flight_requests(),
                );
                self.as_mut().channel().start_send(response)?;
                Poll::Ready(Some(Ok(())))
            }
            Poll::Ready(None) => {
                // Shutdown can't be done before we finish pumping out remaining responses.
                ready!(self.as_mut().channel().poll_flush(cx)?);
                Poll::Ready(None)
            }
            Poll::Pending => {
                // No more requests to process, so flush any requests buffered in the transport.
                ready!(self.as_mut().channel().poll_flush(cx)?);

                // Being here means there are no staged requests and all written responses are
                // fully flushed. So, if the read half is closed and there are no in-flight
                // requests, then we can close the write half.
                if read_half_closed && self.as_mut().channel().in_flight_requests() == 0 {
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
        while let Poll::Pending = self.as_mut().channel().poll_ready(cx)? {
            ready!(self.as_mut().channel().poll_flush(cx)?);
        }

        match ready!(self.as_mut().pending_responses().poll_next(cx)) {
            Some((ctx, response)) => Poll::Ready(Some(Ok((ctx, response)))),
            None => {
                // This branch likely won't happen, since the ResponseHandler is holding a Sender.
                Poll::Ready(None)
            }
        }
    }

    fn handle_request(mut self: Pin<&mut Self>, request: Request<C::Req>) -> io::Result<()> {
        let request_id = request.id;
        let deadline = request.context.deadline;
        let timeout = deadline.as_duration();
        trace!(
            "[{}] Received request with deadline {} (timeout {:?}).",
            request.context.trace_id(),
            format_rfc3339(deadline),
            timeout,
        );
        let ctx = request.context;
        let request = request.message;
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
        let abort_registration = self.as_mut().channel().start_request(request_id);
        let response = Abortable::new(response, abort_registration);
        crate::spawn(response.map(|_| ())).map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "Could not spawn response task. Is shutdown: {}",
                    e.is_shutdown()
                ),
            )
        })?;
        Ok(())
    }
}

impl<C, F, Fut> Future for ResponseHandler<C, F>
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
        .map(|r| r.unwrap_or_else(|e| info!("ResponseHandler errored out: {}", e)))
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
