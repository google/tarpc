use crate::{
    cancellations::{cancellations, CanceledRequests, RequestCancellation},
    context::{SpanExt},
    trace, ClientMessage, Request, Response, Transport,
};
use futures::{
    prelude::*,
    stream::Fuse,
    task::*,
};
use super::in_flight_requests::{AlreadyExistsError, InFlightRequests};
use pin_project::pin_project;
use std::{convert::TryFrom, error::Error, fmt, marker::PhantomData, pin::Pin};
use std::sync::Arc;
use opentelemetry::trace::TraceContextExt;
use tracing::{info_span};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use crate::server::{Channel, ChannelError, Config, ResponseGuard, TrackedRequest};

/// BaseChannel is the standard implementation of a [`Channel`].
///
/// BaseChannel manages a [`Transport`](Transport) of client [`messages`](ClientMessage) and
/// implements a [`Stream`] of [requests](TrackedRequest). See the [`Channel`] documentation for
/// how to use channels.
///
/// Besides requests, the other type of client message handled by `BaseChannel` is [cancellation
/// mssages](ClientMessage::Cancel). `BaseChannel` does not allow direct access to cancellation
/// messages. Instead, it internally handles them by cancelling corresponding requests (removing
/// the corresponding in-flight requests and aborting their handlers).
#[pin_project]
pub struct BaseChannel<Req, Resp, T> {
    config: Config,
    /// Writes responses to the wire and reads requests off the wire.
    #[pin]
    transport: Fuse<T>,
    /// In-flight requests that were dropped by the server before completion.
    #[pin]
    pub(super) canceled_requests: CanceledRequests,
    /// Notifies `canceled_requests` when a request is canceled.
    request_cancellation: RequestCancellation,
    /// Holds data necessary to clean up in-flight requests.
    in_flight_requests: InFlightRequests<()>,
    /// Types the request and response.
    ghost: PhantomData<(fn() -> Req, fn(Resp))>,
}

impl<Req, Resp, T> BaseChannel<Req, Resp, T>
    where
        T: Transport<Response<Resp>, ClientMessage<Req>>,
{
    /// Creates a new channel backed by `transport` and configured with `config`.
    pub fn new(config: Config, transport: T) -> Self {
        let (request_cancellation, canceled_requests) = cancellations();
        BaseChannel {
            config,
            transport: transport.fuse(),
            canceled_requests,
            request_cancellation,
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

    fn in_flight_requests_mut<'a>(self: &'a mut Pin<&mut Self>) -> &'a mut InFlightRequests<()> {
        self.as_mut().project().in_flight_requests
    }

    fn canceled_requests_pin_mut<'a>(
        self: &'a mut Pin<&mut Self>,
    ) -> Pin<&'a mut CanceledRequests> {
        self.as_mut().project().canceled_requests
    }

    fn transport_pin_mut<'a>(self: &'a mut Pin<&mut Self>) -> Pin<&'a mut Fuse<T>> {
        self.as_mut().project().transport
    }

    pub(super) fn start_request(
        mut self: Pin<&mut Self>,
        mut request: Request<Req>,
    ) -> Result<TrackedRequest<Req>, AlreadyExistsError> {
        let span = info_span!(
            "RPC",
            rpc.trace_id = %request.context.trace_id(),
            rpc.deadline = %humantime::format_rfc3339(*request.context.deadline),
            otel.kind = "server",
            otel.name = tracing::field::Empty,
        );
        span.set_context(&request.context);
        request.context.trace_context = trace::Context::try_from(&span).unwrap_or_else(|_| {
            tracing::trace!(
                "OpenTelemetry subscriber not installed; making unsampled \
                            child context."
            );
            request.context.trace_context.new_child()
        });
        let entered = span.enter();
        tracing::info!("ReceiveRequest");
        let start = self.in_flight_requests_mut().start_request(
            request.request_id,
            *request.context.deadline,
            (),
            span.clone(),
        );
        match start {
            Ok(abort_registration) => {
                drop(entered);
                Ok(TrackedRequest {
                    abort_registration,
                    span,
                    response_guard: ResponseGuard {
                        request_id: request.request_id,
                        request_cancellation: self.request_cancellation.clone(),
                        cancel: false,
                    },
                    request,
                })
            }
            Err(AlreadyExistsError) => {
                tracing::trace!("DuplicateRequest");
                Err(AlreadyExistsError)
            }
        }
    }
}

impl<Req, Resp, T> fmt::Debug for BaseChannel<Req, Resp, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BaseChannel")
    }
}

impl<Req, Resp, T> Stream for BaseChannel<Req, Resp, T>
    where
        T: Transport<Response<Resp>, ClientMessage<Req>>,
{
    type Item = Result<TrackedRequest<Req>, ChannelError<T::Error>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        #[derive(Clone, Copy, Debug)]
        enum ReceiverStatus {
            Ready,
            Pending,
            Closed,
        }

        impl ReceiverStatus {
            fn combine(self, other: Self) -> Self {
                use ReceiverStatus::*;
                match (self, other) {
                    (Ready, _) | (_, Ready) => Ready,
                    (Closed, Closed) => Closed,
                    (Pending, Closed) | (Closed, Pending) | (Pending, Pending) => Pending,
                }
            }
        }

        use ReceiverStatus::*;

        loop {
            let cancellation_status = match self.canceled_requests_pin_mut().poll_recv(cx) {
                Poll::Ready(Some(request_id)) => {
                    if let Some(((), span)) = self.in_flight_requests_mut().remove_request(request_id) {
                        let _entered = span.enter();
                        tracing::info!("ResponseCancelled");
                    }
                    Ready
                }
                // Pending cancellations don't block Channel closure, because all they do is ensure
                // the Channel's internal state is cleaned up. But Channel closure also cleans up
                // the Channel state, so there's no reason to wait on a cancellation before
                // closing.
                //
                // Ready(None) can't happen, since `self` holds a Cancellation.
                Poll::Pending | Poll::Ready(None) => Closed,
            };

            let expiration_status = match self.in_flight_requests_mut().poll_expired(cx) {
                // No need to send a response, since the client wouldn't be waiting for one
                // anymore.
                Poll::Ready(Some(_)) => Ready,
                Poll::Ready(None) => Closed,
                Poll::Pending => Pending,
            };

            let request_status = match self
                .transport_pin_mut()
                .poll_next(cx)
                .map_err(|e| ChannelError::Read(Arc::new(e)))?
            {
                Poll::Ready(Some(message)) => match message {
                    ClientMessage::Request(request) => {
                        match self.as_mut().start_request(request) {
                            Ok(request) => return Poll::Ready(Some(Ok(request))),
                            Err(AlreadyExistsError) => {
                                // Instead of closing the channel if a duplicate request is sent,
                                // just ignore it, since it's already being processed. Note that we
                                // cannot return Poll::Pending here, since nothing has scheduled a
                                // wakeup yet.
                                continue;
                            }
                        }
                    }
                    ClientMessage::Cancel {
                        context,
                        request_id,
                    } => {
                        if !self.in_flight_requests_mut().cancel_request(request_id) {
                            tracing::trace!(
                                rpc.trace_id = %context.trace_id,
                                "Received cancellation, but response handler is already complete.",
                            );
                        }
                        Ready
                    }
                },
                Poll::Ready(None) => Closed,
                Poll::Pending => Pending,
            };

            let status = cancellation_status
                .combine(expiration_status)
                .combine(request_status);

            tracing::trace!(
                "Cancellations: {cancellation_status:?}, \
                Expired requests: {expiration_status:?}, \
                Inbound: {request_status:?}, \
                Overall: {status:?}",
            );
            match status {
                Ready => continue,
                Closed => return Poll::Ready(None),
                Pending => return Poll::Pending,
            }
        }
    }
}

impl<Req, Resp, T> Sink<Response<Resp>> for BaseChannel<Req, Resp, T>
    where
        T: Transport<Response<Resp>, ClientMessage<Req>>,
        T::Error: Error,
{
    type Error = ChannelError<T::Error>;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.project()
            .transport
            .poll_ready(cx)
            .map_err(ChannelError::Ready)
    }

    fn start_send(mut self: Pin<&mut Self>, response: Response<Resp>) -> Result<(), Self::Error> {
        if let Some(((), span)) = self
            .in_flight_requests_mut()
            .remove_request(response.request_id)
        {
            let _entered = span.enter();
            tracing::info!("SendResponse");
            self.project()
                .transport
                .start_send(response)
                .map_err(ChannelError::Write)
        } else {
            // If the request isn't tracked anymore, there's no need to send the response.
            Ok(())
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        tracing::trace!("poll_flush");
        self.project()
            .transport
            .poll_flush(cx)
            .map_err(ChannelError::Flush)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        self.project()
            .transport
            .poll_close(cx)
            .map_err(ChannelError::Close)
    }
}

impl<Req, Resp, T> AsRef<T> for BaseChannel<Req, Resp, T> {
    fn as_ref(&self) -> &T {
        self.transport.get_ref()
    }
}

impl<Req, Resp, T>Channel for BaseChannel<Req, Resp, T>
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
}
