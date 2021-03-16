// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a server that concurrently handles many connections sending multiplexed requests.

use crate::{
    context::{self, SpanExt},
    trace, ClientMessage, PollIo, Request, Response, Transport,
};
use futures::{
    future::{AbortRegistration, Abortable},
    prelude::*,
    ready,
    stream::Fuse,
    task::*,
};
use in_flight_requests::{AlreadyExistsError, InFlightRequests};
use pin_project::pin_project;
use std::{convert::TryFrom, fmt, hash::Hash, io, marker::PhantomData, pin::Pin, time::SystemTime};
use tokio::sync::mpsc;
use tracing::{info_span, instrument::Instrument, Span};

mod filter;
mod in_flight_requests;
#[cfg(test)]
mod testing;
mod throttle;

pub use self::{
    filter::ChannelFilter,
    throttle::{Throttler, ThrottlerStream},
};

/// Settings that control the behavior of [channels](Channel).
#[derive(Clone, Debug)]
pub struct Config {
    /// Controls the buffer size of the in-process channel over which a server's handlers send
    /// responses to the [`Channel`]. In other words, this is the number of responses that can sit
    /// in the outbound queue before request handlers begin blocking.
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
        T: Transport<Response<Resp>, ClientMessage<Req>>,
    {
        BaseChannel::new(self, transport)
    }
}

/// Equivalent to a `FnOnce(Req) -> impl Future<Output = Resp>`.
pub trait Serve<Req> {
    /// Type of response.
    type Resp;

    /// Type of response future.
    type Fut: Future<Output = Self::Resp>;

    /// Extracts a method name from the request.
    fn method(&self, _request: &Req) -> Option<&'static str> {
        None
    }

    /// Responds to a single request.
    fn serve(self, ctx: context::Context, req: Req) -> Self::Fut;
}

impl<Req, Resp, Fut, F> Serve<Req> for F
where
    F: FnOnce(context::Context, Req) -> Fut,
    Fut: Future<Output = Resp>,
{
    type Resp = Resp;
    type Fut = Fut;

    fn serve(self, ctx: context::Context, req: Req) -> Self::Fut {
        self(ctx, req)
    }
}

/// An extension trait for [streams](Stream) of [`Channels`](Channel).
pub trait Incoming<C>
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

    /// [Executes](Channel::execute) each incoming channel. Each channel will be handled
    /// concurrently by spawning on tokio's default executor, and each request will be also
    /// be spawned on tokio's default executor.
    #[cfg(feature = "tokio1")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tokio1")))]
    fn execute<S>(self, serve: S) -> TokioServerExecutor<Self, S>
    where
        S: Serve<C::Req, Resp = C::Resp>,
    {
        TokioServerExecutor { inner: self, serve }
    }
}

impl<S, C> Incoming<C> for S
where
    S: Sized + Stream<Item = C>,
    C: Channel,
{
}

/// BaseChannel is a [Transport] that keeps track of in-flight requests. It converts a
/// [`Transport`](Transport) of [`ClientMessages`](ClientMessage) into a stream of
/// [requests](ClientMessage::Request).
///
/// Besides requests, the other type of client message is [cancellation
/// messages](ClientMessage::Cancel). `BaseChannel` does not allow direct access to cancellation
/// messages. Instead, it internally handles them by cancelling corresponding requests (removing
/// the corresponding in-flight requests and aborting their handlers).
#[pin_project]
pub struct BaseChannel<Req, Resp, T> {
    config: Config,
    /// Writes responses to the wire and reads requests off the wire.
    #[pin]
    transport: Fuse<T>,
    /// Holds data necessary to clean up in-flight requests.
    in_flight_requests: InFlightRequests,
    /// Types the request and response.
    ghost: PhantomData<(Req, Resp)>,
}

impl<Req, Resp, T> BaseChannel<Req, Resp, T>
where
    T: Transport<Response<Resp>, ClientMessage<Req>>,
{
    /// Creates a new channel backed by `transport` and configured with `config`.
    pub fn new(config: Config, transport: T) -> Self {
        BaseChannel {
            config,
            transport: transport.fuse(),
            in_flight_requests: InFlightRequests::default(),
            ghost: PhantomData,
        }
    }

    /// Creates a new channel backed by `transport` and configured with the defaults.
    pub fn with_defaults(transport: T) -> Self {
        Self::new(Config::default(), transport)
    }

    /// Returns the inner transport over which messages are sent and received.
    pub fn get_ref(&self) -> &T {
        self.transport.get_ref()
    }

    /// Returns the inner transport over which messages are sent and received.
    pub fn get_pin_ref(self: Pin<&mut Self>) -> Pin<&mut T> {
        self.project().transport.get_pin_mut()
    }

    fn in_flight_requests_mut<'a>(self: &'a mut Pin<&mut Self>) -> &'a mut InFlightRequests {
        self.as_mut().project().in_flight_requests
    }

    fn transport_pin_mut<'a>(self: &'a mut Pin<&mut Self>) -> Pin<&'a mut Fuse<T>> {
        self.as_mut().project().transport
    }
}

impl<Req, Resp, T> fmt::Debug for BaseChannel<Req, Resp, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BaseChannel")
    }
}

/// The server end of an open connection with a client, streaming in requests from, and sinking
/// responses to, the client.
///
/// The ways to use a Channel, in order of simplest to most complex, is:
/// 1. [`Channel::execute`] - Requires the `tokio1` feature. This method is best for those who
///    do not have specific scheduling needs and whose services are `Send + 'static`.
/// 2. [`Channel::requests`] - This method is best for those who need direct access to individual
///    requests, or are not using `tokio`, or want control over [futures](Future) scheduling.
///    [`Requests`] is a stream of [`InFlightRequests`](InFlightRequest), each which has an
///    [`execute`](InFlightRequest::execute) method. If using `execute`, request processing will
///    automatically cease when either the request deadline is reached or when a corresponding
///    cancellation message is received by the Channel.
/// 3. [`Sink::start_send`] - A user is free to manually send responses to requests produced by a
///    Channel using [`Sink::start_send`] in lieu of the previous methods. If not using one of the
///    previous execute methods, then nothing will automatically cancel requests or set up the
///    request context. However, the Channel will still clean up resources upon deadline expiration
///    or cancellation. In the case that the Channel cleans up resources related to a request
///    before the response is sent, the response can still be sent into the Channel later on.
///    Because there is no guarantee that a cancellation message will ever be received for a
///    request, or that requests come with reasonably short deadlines, services should strive to
///    clean up Channel resources by sending a response for every request.
pub trait Channel
where
    Self: Transport<Response<<Self as Channel>::Resp>, Request<<Self as Channel>::Req>>,
{
    /// Type of request item.
    type Req;

    /// Type of response sink item.
    type Resp;

    /// The wrapped transport.
    type Transport;

    /// Configuration of the channel.
    fn config(&self) -> &Config;

    /// Returns the number of in-flight requests over this channel.
    fn in_flight_requests(&self) -> usize;

    /// Returns the transport underlying the channel.
    fn transport(&self) -> &Self::Transport;

    /// Caps the number of concurrent requests to `limit`.
    fn max_concurrent_requests(self, limit: usize) -> Throttler<Self>
    where
        Self: Sized,
    {
        Throttler::new(self, limit)
    }

    /// Tells the Channel that request with ID `request_id` is being handled.
    /// The request will be tracked until a response with the same ID is sent
    /// to the Channel.
    fn start_request(
        self: Pin<&mut Self>,
        id: u64,
        deadline: SystemTime,
        span: Span,
    ) -> Result<AbortRegistration, AlreadyExistsError>;

    /// Returns a stream of requests that automatically handle request cancellation and response
    /// routing.
    ///
    /// This is a terminal operation. After calling `requests`, the channel cannot be retrieved,
    /// and the only way to complete requests is via [`Requests::execute`] or
    /// [`InFlightRequest::execute`].
    fn requests(self) -> Requests<Self>
    where
        Self: Sized,
    {
        let (responses_tx, responses) = mpsc::channel(self.config().pending_response_buffer);

        Requests {
            channel: self,
            pending_responses: responses,
            responses_tx,
        }
    }

    /// Runs the channel until completion by executing all requests using the given service
    /// function. Request handlers are run concurrently by [spawning](tokio::spawn) on tokio's
    /// default executor.
    #[cfg(feature = "tokio1")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tokio1")))]
    fn execute<S>(self, serve: S) -> TokioChannelExecutor<Requests<Self>, S>
    where
        Self: Sized,
        S: Serve<Self::Req, Resp = Self::Resp> + Send + 'static,
        S::Fut: Send,
        Self::Req: Send + 'static,
        Self::Resp: Send + 'static,
    {
        self.requests().execute(serve)
    }
}

impl<Req, Resp, T> Stream for BaseChannel<Req, Resp, T>
where
    T: Transport<Response<Resp>, ClientMessage<Req>>,
{
    type Item = io::Result<Request<Req>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        enum ReceiverStatus {
            Ready,
            Pending,
            Closed,
        }
        use ReceiverStatus::*;

        loop {
            let expiration_status = match self.in_flight_requests_mut().poll_expired(cx)? {
                // No need to send a response, since the client wouldn't be waiting for one
                // anymore.
                Poll::Ready(Some(_)) => Ready,
                Poll::Ready(None) => Closed,
                Poll::Pending => Pending,
            };

            let request_status = match self.transport_pin_mut().poll_next(cx)? {
                Poll::Ready(Some(message)) => match message {
                    ClientMessage::Request(request) => {
                        return Poll::Ready(Some(Ok(request)));
                    }
                    ClientMessage::Cancel {
                        trace_context,
                        request_id,
                    } => {
                        if !self.in_flight_requests_mut().cancel_request(request_id) {
                            tracing::trace!(
                                rpc.trace_id = %trace_context.trace_id,
                                "Received cancellation, but response handler is already complete.",
                            );
                        }
                        Ready
                    }
                },
                Poll::Ready(None) => Closed,
                Poll::Pending => Pending,
            };

            match (expiration_status, request_status) {
                (Ready, _) | (_, Ready) => continue,
                (Closed, Closed) => return Poll::Ready(None),
                (Pending, Closed) | (Closed, Pending) | (Pending, Pending) => return Poll::Pending,
            }
        }
    }
}

impl<Req, Resp, T> Sink<Response<Resp>> for BaseChannel<Req, Resp, T>
where
    T: Transport<Response<Resp>, ClientMessage<Req>>,
{
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.project().transport.poll_ready(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, response: Response<Resp>) -> Result<(), Self::Error> {
        if let Some(span) = self
            .as_mut()
            .project()
            .in_flight_requests
            .remove_request(response.request_id)
        {
            let _entered = span.enter();
            tracing::info!("SendResponse");
            self.project().transport.start_send(response)
        } else {
            // If the request isn't tracked anymore, there's no need to send the response.
            Ok(())
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.project().transport.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.project().transport.poll_close(cx)
    }
}

impl<Req, Resp, T> AsRef<T> for BaseChannel<Req, Resp, T> {
    fn as_ref(&self) -> &T {
        self.transport.get_ref()
    }
}

impl<Req, Resp, T> Channel for BaseChannel<Req, Resp, T>
where
    T: Transport<Response<Resp>, ClientMessage<Req>>,
{
    type Req = Req;
    type Resp = Resp;
    type Transport = T;

    fn config(&self) -> &Config {
        &self.config
    }

    fn in_flight_requests(&self) -> usize {
        self.in_flight_requests.len()
    }

    fn transport(&self) -> &Self::Transport {
        self.get_ref()
    }

    fn start_request(
        self: Pin<&mut Self>,
        id: u64,
        deadline: SystemTime,
        span: Span,
    ) -> Result<AbortRegistration, AlreadyExistsError> {
        self.project()
            .in_flight_requests
            .start_request(id, deadline, span)
    }
}

/// A stream of requests coming over a channel. `Requests` also drives the sending of responses, so
/// it must be continually polled to ensure progress.
#[pin_project]
pub struct Requests<C>
where
    C: Channel,
{
    #[pin]
    channel: C,
    /// Responses waiting to be written to the wire.
    pending_responses: mpsc::Receiver<Response<C::Resp>>,
    /// Handed out to request handlers to fan in responses.
    responses_tx: mpsc::Sender<Response<C::Resp>>,
}

impl<C> Requests<C>
where
    C: Channel,
{
    /// Returns the inner channel over which messages are sent and received.
    pub fn channel_pin_mut<'a>(self: &'a mut Pin<&mut Self>) -> Pin<&'a mut C> {
        self.as_mut().project().channel
    }

    /// Returns the inner channel over which messages are sent and received.
    pub fn pending_responses_mut<'a>(
        self: &'a mut Pin<&mut Self>,
    ) -> &'a mut mpsc::Receiver<Response<C::Resp>> {
        self.as_mut().project().pending_responses
    }

    fn pump_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> PollIo<InFlightRequest<C::Req, C::Resp>> {
        loop {
            match ready!(self.channel_pin_mut().poll_next(cx)?) {
                Some(mut request) => {
                    let span = info_span!(
                        "RPC",
                        rpc.trace_id = %request.context.trace_id(),
                        otel.kind = "server",
                        otel.name = tracing::field::Empty,
                    );
                    span.set_context(&request.context);
                    request.context.trace_context =
                        trace::Context::try_from(&span).unwrap_or_else(|_| {
                            tracing::trace!(
                                "OpenTelemetry subscriber not installed; making unsampled
                                        child context."
                            );
                            request.context.trace_context.new_child()
                        });
                    let entered = span.enter();
                    tracing::info!("ReceiveRequest");
                    let start = self.channel_pin_mut().start_request(
                        request.id,
                        request.context.deadline,
                        span.clone(),
                    );
                    match start {
                        Ok(abort_registration) => {
                            let response_tx = self.responses_tx.clone();
                            drop(entered);
                            return Poll::Ready(Some(Ok(InFlightRequest {
                                request,
                                response_tx,
                                abort_registration,
                                span,
                            })));
                        }
                        // Instead of closing the channel if a duplicate request is sent, just
                        // ignore it, since it's already being processed. Note that we cannot
                        // return Poll::Pending here, since nothing has scheduled a wakeup yet.
                        Err(AlreadyExistsError) => {
                            tracing::trace!("DuplicateRequest");
                            continue;
                        }
                    }
                }
                None => return Poll::Ready(None),
            }
        }
    }

    fn pump_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        read_half_closed: bool,
    ) -> PollIo<()> {
        match self.as_mut().poll_next_response(cx)? {
            Poll::Ready(Some(response)) => {
                // A Ready result from poll_next_response means the Channel is ready to be written
                // to. Therefore, we can call start_send without worry of a full buffer.
                self.channel_pin_mut().start_send(response)?;
                Poll::Ready(Some(Ok(())))
            }
            Poll::Ready(None) => {
                // Shutdown can't be done before we finish pumping out remaining responses.
                ready!(self.channel_pin_mut().poll_flush(cx)?);
                Poll::Ready(None)
            }
            Poll::Pending => {
                // No more requests to process, so flush any requests buffered in the transport.
                ready!(self.channel_pin_mut().poll_flush(cx)?);

                // Being here means there are no staged requests and all written responses are
                // fully flushed. So, if the read half is closed and there are no in-flight
                // requests, then we can close the write half.
                if read_half_closed && self.channel.in_flight_requests() == 0 {
                    Poll::Ready(None)
                } else {
                    Poll::Pending
                }
            }
        }
    }

    /// Yields a response ready to be written to the Channel sink.
    ///
    /// Note that a response will only be yielded if the Channel is *ready* to be written to (i.e.
    /// start_send would succeed).
    fn poll_next_response(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> PollIo<Response<C::Resp>> {
        ready!(self.ensure_writeable(cx)?);

        match ready!(self.pending_responses_mut().poll_recv(cx)) {
            Some(response) => Poll::Ready(Some(Ok(response))),
            None => {
                // This branch likely won't happen, since the Requests stream is holding a Sender.
                Poll::Ready(None)
            }
        }
    }

    /// Returns Ready if writing a message to the Channel would not fail due to a full buffer. If
    /// the Channel is not ready to be written to, flushes it until it is ready.
    fn ensure_writeable<'a>(self: &'a mut Pin<&mut Self>, cx: &mut Context<'_>) -> PollIo<()> {
        while self.channel_pin_mut().poll_ready(cx)?.is_pending() {
            ready!(self.channel_pin_mut().poll_flush(cx)?);
        }
        Poll::Ready(Some(Ok(())))
    }
}

impl<C> fmt::Debug for Requests<C>
where
    C: Channel,
{
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "Requests")
    }
}

/// A request produced by [Channel::requests].
#[derive(Debug)]
pub struct InFlightRequest<Req, Res> {
    request: Request<Req>,
    response_tx: mpsc::Sender<Response<Res>>,
    abort_registration: AbortRegistration,
    span: Span,
}

impl<Req, Res> InFlightRequest<Req, Res> {
    /// Returns a reference to the request.
    pub fn get(&self) -> &Request<Req> {
        &self.request
    }

    /// Returns a [future](Future) that executes the request using the given [service
    /// function](Serve). The service function's output is automatically sent back to the [Channel]
    /// that yielded this request. The request will be executed in the scope of this request's
    /// context.
    ///
    /// The returned future will stop executing when the first of the following conditions is met:
    ///
    /// 1. The channel that yielded this request receives a [cancellation
    ///    message](ClientMessage::Cancel) for this request.
    /// 2. The request [deadline](crate::context::Context::deadline) is reached.
    /// 3. The service function completes.
    pub async fn execute<S>(self, serve: S)
    where
        S: Serve<Req, Resp = Res>,
    {
        let Self {
            abort_registration,
            request:
                Request {
                    context,
                    message,
                    id: request_id,
                },
            response_tx,
            span,
        } = self;
        let method = serve.method(&message);
        span.record("otel.name", &method.unwrap_or(""));
        let _ = Abortable::new(
            async move {
                let response = serve.serve(context, message).await;
                tracing::info!("CompleteRequest");
                let response = Response {
                    request_id,
                    message: Ok(response),
                };
                let _ = response_tx.send(response).await;
                tracing::info!("BufferResponse");
            },
            abort_registration,
        )
        .instrument(span)
        .await;
    }
}

impl<C> Stream for Requests<C>
where
    C: Channel,
{
    type Item = io::Result<InFlightRequest<C::Req, C::Resp>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let read = self.as_mut().pump_read(cx)?;
            let read_closed = matches!(read, Poll::Ready(None));
            match (read, self.as_mut().pump_write(cx, read_closed)?) {
                (Poll::Ready(None), Poll::Ready(None)) => {
                    return Poll::Ready(None);
                }
                (Poll::Ready(Some(request_handler)), _) => {
                    return Poll::Ready(Some(Ok(request_handler)));
                }
                (_, Poll::Ready(Some(()))) => {}
                _ => {
                    return Poll::Pending;
                }
            }
        }
    }
}

// Send + 'static execution helper methods.

#[cfg(feature = "tokio1")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio1")))]
impl<C> Requests<C>
where
    C: Channel,
    C::Req: Send + 'static,
    C::Resp: Send + 'static,
{
    /// Executes all requests using the given service function. Requests are handled concurrently
    /// by [spawning](tokio::spawn) each handler on tokio's default executor.
    pub fn execute<S>(self, serve: S) -> TokioChannelExecutor<Self, S>
    where
        S: Serve<C::Req, Resp = C::Resp> + Send + 'static,
    {
        TokioChannelExecutor { inner: self, serve }
    }
}

/// A future that drives the server by [spawning](tokio::spawn) a [`TokioChannelExecutor`](TokioChannelExecutor)
/// for each new channel.
#[pin_project]
#[derive(Debug)]
#[cfg(feature = "tokio1")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio1")))]
pub struct TokioServerExecutor<T, S> {
    #[pin]
    inner: T,
    serve: S,
}

/// A future that drives the server by [spawning](tokio::spawn) each [response
/// handler](InFlightRequest::execute) on tokio's default executor.
#[pin_project]
#[derive(Debug)]
#[cfg(feature = "tokio1")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio1")))]
pub struct TokioChannelExecutor<T, S> {
    #[pin]
    inner: T,
    serve: S,
}

#[cfg(feature = "tokio1")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio1")))]
impl<T, S> TokioServerExecutor<T, S> {
    fn inner_pin_mut<'a>(self: &'a mut Pin<&mut Self>) -> Pin<&'a mut T> {
        self.as_mut().project().inner
    }
}

#[cfg(feature = "tokio1")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio1")))]
impl<T, S> TokioChannelExecutor<T, S> {
    fn inner_pin_mut<'a>(self: &'a mut Pin<&mut Self>) -> Pin<&'a mut T> {
        self.as_mut().project().inner
    }
}

#[cfg(feature = "tokio1")]
impl<St, C, Se> Future for TokioServerExecutor<St, Se>
where
    St: Sized + Stream<Item = C>,
    C: Channel + Send + 'static,
    C::Req: Send + 'static,
    C::Resp: Send + 'static,
    Se: Serve<C::Req, Resp = C::Resp> + Send + 'static + Clone,
    Se::Fut: Send,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        while let Some(channel) = ready!(self.inner_pin_mut().poll_next(cx)) {
            tokio::spawn(channel.execute(self.serve.clone()));
        }
        tracing::info!("Server shutting down.");
        Poll::Ready(())
    }
}

#[cfg(feature = "tokio1")]
impl<C, S> Future for TokioChannelExecutor<Requests<C>, S>
where
    C: Channel + 'static,
    C::Req: Send + 'static,
    C::Resp: Send + 'static,
    S: Serve<C::Req, Resp = C::Resp> + Send + 'static + Clone,
    S::Fut: Send,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        while let Some(response_handler) = ready!(self.inner_pin_mut().poll_next(cx)) {
            match response_handler {
                Ok(resp) => {
                    let server = self.serve.clone();
                    tokio::spawn(async move {
                        resp.execute(server).await;
                    });
                }
                Err(e) => {
                    tracing::info!("Requests stream errored out: {}", e);
                    break;
                }
            }
        }
        Poll::Ready(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{
        trace,
        transport::channel::{self, UnboundedChannel},
    };
    use assert_matches::assert_matches;
    use futures::future::{pending, Aborted};
    use futures_test::task::noop_context;
    use std::time::Duration;

    fn test_channel<Req, Resp>() -> (
        Pin<Box<BaseChannel<Req, Resp, UnboundedChannel<ClientMessage<Req>, Response<Resp>>>>>,
        UnboundedChannel<Response<Resp>, ClientMessage<Req>>,
    ) {
        let (tx, rx) = crate::transport::channel::unbounded();
        (Box::pin(BaseChannel::new(Config::default(), rx)), tx)
    }

    fn test_requests<Req, Resp>() -> (
        Pin<
            Box<
                Requests<
                    BaseChannel<Req, Resp, UnboundedChannel<ClientMessage<Req>, Response<Resp>>>,
                >,
            >,
        >,
        UnboundedChannel<Response<Resp>, ClientMessage<Req>>,
    ) {
        let (tx, rx) = crate::transport::channel::unbounded();
        (
            Box::pin(BaseChannel::new(Config::default(), rx).requests()),
            tx,
        )
    }

    fn test_bounded_requests<Req, Resp>(
        capacity: usize,
    ) -> (
        Pin<
            Box<
                Requests<
                    BaseChannel<Req, Resp, channel::Channel<ClientMessage<Req>, Response<Resp>>>,
                >,
            >,
        >,
        channel::Channel<Response<Resp>, ClientMessage<Req>>,
    ) {
        let (tx, rx) = crate::transport::channel::bounded(capacity);
        let mut config = Config::default();
        // Add 1 because capacity 0 is not supported (but is supported by transport::channel::bounded).
        config.pending_response_buffer = capacity + 1;
        (Box::pin(BaseChannel::new(config, rx).requests()), tx)
    }

    fn fake_request<Req>(req: Req) -> ClientMessage<Req> {
        ClientMessage::Request(Request {
            context: context::current(),
            id: 0,
            message: req,
        })
    }

    fn test_abortable(
        abort_registration: AbortRegistration,
    ) -> impl Future<Output = Result<(), Aborted>> {
        Abortable::new(pending(), abort_registration)
    }

    #[tokio::test]
    async fn base_channel_start_send_duplicate_request_returns_error() {
        let (mut channel, _tx) = test_channel::<(), ()>();

        channel
            .as_mut()
            .start_request(0, SystemTime::now(), Span::current())
            .unwrap();
        assert_matches!(
            channel
                .as_mut()
                .start_request(0, SystemTime::now(), Span::current()),
            Err(AlreadyExistsError)
        );
    }

    #[tokio::test]
    async fn base_channel_poll_next_aborts_multiple_requests() {
        let (mut channel, _tx) = test_channel::<(), ()>();

        tokio::time::pause();
        let abort_registration0 = channel
            .as_mut()
            .start_request(0, SystemTime::now(), Span::current())
            .unwrap();
        let abort_registration1 = channel
            .as_mut()
            .start_request(1, SystemTime::now(), Span::current())
            .unwrap();
        tokio::time::advance(std::time::Duration::from_secs(1000)).await;

        assert_matches!(
            channel.as_mut().poll_next(&mut noop_context()),
            Poll::Pending
        );
        assert_matches!(test_abortable(abort_registration0).await, Err(Aborted));
        assert_matches!(test_abortable(abort_registration1).await, Err(Aborted));
    }

    #[tokio::test]
    async fn base_channel_poll_next_aborts_canceled_request() {
        let (mut channel, mut tx) = test_channel::<(), ()>();

        tokio::time::pause();
        let abort_registration = channel
            .as_mut()
            .start_request(
                0,
                SystemTime::now() + Duration::from_millis(100),
                Span::current(),
            )
            .unwrap();

        tx.send(ClientMessage::Cancel {
            trace_context: trace::Context::default(),
            request_id: 0,
        })
        .await
        .unwrap();

        assert_matches!(
            channel.as_mut().poll_next(&mut noop_context()),
            Poll::Pending
        );

        assert_matches!(test_abortable(abort_registration).await, Err(Aborted));
    }

    #[tokio::test]
    async fn base_channel_with_closed_transport_and_in_flight_request_returns_pending() {
        let (mut channel, tx) = test_channel::<(), ()>();

        tokio::time::pause();
        let _abort_registration = channel
            .as_mut()
            .start_request(
                0,
                SystemTime::now() + Duration::from_millis(100),
                Span::current(),
            )
            .unwrap();

        drop(tx);
        assert_matches!(
            channel.as_mut().poll_next(&mut noop_context()),
            Poll::Pending
        );
    }

    #[tokio::test]
    async fn base_channel_with_closed_transport_and_no_in_flight_requests_returns_closed() {
        let (mut channel, tx) = test_channel::<(), ()>();
        drop(tx);
        assert_matches!(
            channel.as_mut().poll_next(&mut noop_context()),
            Poll::Ready(None)
        );
    }

    #[tokio::test]
    async fn base_channel_poll_next_yields_request() {
        let (mut channel, mut tx) = test_channel::<(), ()>();
        tx.send(fake_request(())).await.unwrap();

        assert_matches!(
            channel.as_mut().poll_next(&mut noop_context()),
            Poll::Ready(Some(Ok(_)))
        );
    }

    #[tokio::test]
    async fn base_channel_poll_next_aborts_request_and_yields_request() {
        let (mut channel, mut tx) = test_channel::<(), ()>();

        tokio::time::pause();
        let abort_registration = channel
            .as_mut()
            .start_request(0, SystemTime::now(), Span::current())
            .unwrap();
        tokio::time::advance(std::time::Duration::from_secs(1000)).await;

        tx.send(fake_request(())).await.unwrap();

        assert_matches!(
            channel.as_mut().poll_next(&mut noop_context()),
            Poll::Ready(Some(Ok(_)))
        );
        assert_matches!(test_abortable(abort_registration).await, Err(Aborted));
    }

    #[tokio::test]
    async fn base_channel_start_send_removes_in_flight_request() {
        let (mut channel, _tx) = test_channel::<(), ()>();

        channel
            .as_mut()
            .start_request(0, SystemTime::now(), Span::current())
            .unwrap();
        assert_eq!(channel.in_flight_requests(), 1);
        channel
            .as_mut()
            .start_send(Response {
                request_id: 0,
                message: Ok(()),
            })
            .unwrap();
        assert_eq!(channel.in_flight_requests(), 0);
    }

    #[tokio::test]
    async fn requests_poll_next_response_returns_pending_when_buffer_full() {
        let (mut requests, _tx) = test_bounded_requests::<(), ()>(0);

        // Response written to the transport.
        requests
            .as_mut()
            .channel_pin_mut()
            .start_request(0, SystemTime::now(), Span::current())
            .unwrap();
        requests
            .as_mut()
            .channel_pin_mut()
            .start_send(Response {
                request_id: 0,
                message: Ok(()),
            })
            .unwrap();

        // Response waiting to be written.
        requests
            .as_mut()
            .project()
            .responses_tx
            .send(Response {
                request_id: 1,
                message: Ok(()),
            })
            .await
            .unwrap();

        requests
            .as_mut()
            .channel_pin_mut()
            .start_request(1, SystemTime::now(), Span::current())
            .unwrap();

        assert_matches!(
            requests.as_mut().poll_next_response(&mut noop_context()),
            Poll::Pending
        );
    }

    #[tokio::test]
    async fn requests_pump_write_returns_pending_when_buffer_full() {
        let (mut requests, _tx) = test_bounded_requests::<(), ()>(0);

        // Response written to the transport.
        requests
            .as_mut()
            .channel_pin_mut()
            .start_request(0, SystemTime::now(), Span::current())
            .unwrap();
        requests
            .as_mut()
            .channel_pin_mut()
            .start_send(Response {
                request_id: 0,
                message: Ok(()),
            })
            .unwrap();

        // Response waiting to be written.
        requests
            .as_mut()
            .channel_pin_mut()
            .start_request(1, SystemTime::now(), Span::current())
            .unwrap();
        requests
            .as_mut()
            .project()
            .responses_tx
            .send(Response {
                request_id: 1,
                message: Ok(()),
            })
            .await
            .unwrap();

        assert_matches!(
            requests.as_mut().pump_write(&mut noop_context(), true),
            Poll::Pending
        );
        // Assert that the pending response was not polled while the channel was blocked.
        assert_matches!(
            requests.as_mut().pending_responses_mut().recv().await,
            Some(_)
        );
    }

    #[tokio::test]
    async fn requests_pump_read() {
        let (mut requests, mut tx) = test_requests::<(), ()>();

        // Response written to the transport.
        tx.send(fake_request(())).await.unwrap();

        assert_matches!(
            requests.as_mut().pump_read(&mut noop_context()),
            Poll::Ready(Some(Ok(_)))
        );
        assert_eq!(requests.channel.in_flight_requests(), 1);
    }
}
