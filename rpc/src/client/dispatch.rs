use crate::util::Compact;
use crate::Request;
use crate::Response;
use crate::Transport;
use fnv::FnvHashMap;
use futures::prelude::*;
use futures::{
    channel::{
        mpsc,
        oneshot::{self, Canceled},
    },
    stream::Fuse,
    task,
};
use pin_utils::unsafe_pinned;
use std::pin::PinMut;
use std::{io, net::SocketAddr};

use super::Config;
use crate::client;
use crate::ClientMessage;
use crate::ClientMessageKind;

/// Handles communication from the client to request dispatch.
#[derive(Debug)]
pub struct Channel<Req, Resp> {
    to_dispatch: mpsc::Sender<DispatchRequest<Req, Resp>>,
    /// Channel to send a cancel message to the dispatcher.
    cancellation: RequestCancellation,
}

impl<Req, Resp> Clone for Channel<Req, Resp> {
    fn clone(&self) -> Self {
        Self {
            to_dispatch: self.to_dispatch.clone(),
            cancellation: self.cancellation.clone(),
        }
    }
}

impl<Req, Resp> Channel<Req, Resp> {
    /// Sends a request to the dispatch task to forward to the server, returning a [`Future`] that
    /// resolves to the response.
    async fn start_send(
        &mut self,
        context: client::Context,
        request_id: u64,
        request: Req,
    ) -> io::Result<DispatchResponse<Resp>> {
        let (response_completion, response) = oneshot::channel();
        let cancellation = self.cancellation.clone();
        await!(self.to_dispatch.send(DispatchRequest {
            context,
            request_id,
            request,
            response_completion,
        })).map_err(|_| io::Error::from(io::ErrorKind::ConnectionReset))?;
        Ok(DispatchResponse {
            response,
            complete: false,
            request_id,
            cancellation,
        })
    }

    /// Sends a request to the dispatch task to forward to the server, returning a [`Future`] that
    /// resolves to the response.
    crate async fn send(
        &mut self,
        context: client::Context,
        request_id: u64,
        request: Req,
    ) -> io::Result<Response<Resp>> {
        let response_future = await!(self.start_send(context, request_id, request))?;
        await!(response_future)
    }
}

/// A server response that is completed by request dispatch when the corresponding response
/// arrives off the wire.
#[derive(Debug)]
pub struct DispatchResponse<Resp> {
    response: oneshot::Receiver<Response<Resp>>,
    complete: bool,
    cancellation: RequestCancellation,
    request_id: u64,
}

impl<Resp> Future for DispatchResponse<Resp> {
    type Output = io::Result<Response<Resp>>;

    fn poll(mut self: PinMut<Self>, cx: &mut task::Context) -> Poll<io::Result<Response<Resp>>> {
        let resp = try_ready!(
            self.response
                .poll_unpin(cx)
                .map_err(|Canceled| io::Error::from(io::ErrorKind::ConnectionReset))
        );

        self.complete = true;

        Poll::Ready(Ok(resp))
    }
}

// Cancels the request when dropped, if not already complete.
impl<Resp> Drop for DispatchResponse<Resp> {
    fn drop(&mut self) {
        if !self.complete {
            // The receiver needs to be closed to handle the edge case that the request has not
            // yet been received by the dispatch task. It is possible for the cancel message to
            // arrive before the request itself, in which case the request could get stuck in the
            // dispatch map forever if the server never responds (e.g. if the server dies while
            // responding). Even if the server does respond, it will have unnecessarily done work
            // for a client no longer waiting for a response. To avoid this, the dispatch task
            // checks if the receiver is closed before inserting the request in the map. By
            // closing the receiver before sending the cancel message, it is guaranteed that if the
            // dispatch task misses an early-arriving cancellation message, then it will see the
            // receiver as closed.
            self.response.close();
            self.cancellation.cancel(self.request_id);
        }
    }
}

/// Spawns a dispatch task on the default executor that manages the lifecycle of requests initiated
/// by the returned [`Channel`].
pub async fn spawn<Req, Resp, C>(
    config: Config,
    transport: C,
    server_addr: SocketAddr,
) -> Channel<Req, Resp>
where
    Req: Send,
    Resp: Send,
    C: Transport<Item = Response<Resp>, SinkItem = ClientMessage<Req>> + Send,
{
    let (to_dispatch, pending_requests) = mpsc::channel(config.pending_request_buffer);
    let (cancellation, canceled_requests) = cancellations();

    spawn!(
        RequestDispatch {
            config,
            server_addr,
            canceled_requests,
            transport: transport.fuse(),
            in_flight_requests: FnvHashMap::default(),
            pending_requests: pending_requests.fuse(),
        }.unwrap_or_else(move |e| error!("[{}] Connection broken: {}", server_addr, e))
    );

    Channel {
        to_dispatch,
        cancellation,
    }
}

/// Handles the lifecycle of requests, writing requests to the wire, managing cancellations,
/// and dispatching responses to the appropriate channel.
struct RequestDispatch<Req, Resp, C> {
    /// Writes requests to the wire and reads responses off the wire.
    transport: Fuse<C>,
    /// Requests waiting to be written to the wire.
    pending_requests: Fuse<mpsc::Receiver<DispatchRequest<Req, Resp>>>,
    /// Requests that were dropped.
    canceled_requests: CanceledRequests,
    /// Requests already written to the wire that haven't yet received responses.
    in_flight_requests: FnvHashMap<u64, InFlightData<Resp>>,
    /// Configures limits to prevent unlimited resource usage.
    config: Config,
    /// The address of the server connected to.
    server_addr: SocketAddr,
}

impl<Req, Resp, C> RequestDispatch<Req, Resp, C>
where
    Req: Send,
    Resp: Send,
    C: Transport<Item = Response<Resp>, SinkItem = ClientMessage<Req>>,
{
    unsafe_pinned!(server_addr: SocketAddr);
    unsafe_pinned!(in_flight_requests: FnvHashMap<u64, InFlightData<Resp>>);
    unsafe_pinned!(canceled_requests: CanceledRequests);
    unsafe_pinned!(pending_requests: Fuse<mpsc::Receiver<DispatchRequest<Req, Resp>>>);
    unsafe_pinned!(transport: Fuse<C>);

    fn pump_read(self: &mut PinMut<Self>, cx: &mut task::Context) -> Poll<Option<io::Result<()>>> {
        Poll::Ready(match ready!(self.transport().poll_next(cx)?) {
            Some(response) => {
                self.complete(response);
                Some(Ok(()))
            }
            None => {
                trace!("[{}] read half closed", self.server_addr());
                None
            }
        })
    }

    fn pump_write(self: &mut PinMut<Self>, cx: &mut task::Context) -> Poll<Option<io::Result<()>>> {
        enum ReceiverStatus {
            NotReady,
            Closed,
        }

        let pending_requests_status = match self.poll_next_request(cx)? {
            Poll::Ready(Some(dispatch_request)) => {
                self.write_request(dispatch_request)?;
                return Poll::Ready(Some(Ok(())));
            }
            Poll::Ready(None) => ReceiverStatus::Closed,
            Poll::Pending => ReceiverStatus::NotReady,
        };

        let canceled_requests_status = match self.poll_next_cancellation(cx)? {
            Poll::Ready(Some((context, request_id))) => {
                self.write_cancel(context, request_id)?;
                return Poll::Ready(Some(Ok(())));
            }
            Poll::Ready(None) => ReceiverStatus::Closed,
            Poll::Pending => ReceiverStatus::NotReady,
        };

        match (pending_requests_status, canceled_requests_status) {
            (ReceiverStatus::Closed, ReceiverStatus::Closed) => {
                ready!(self.transport().poll_flush(cx)?);
                Poll::Ready(None)
            }
            (ReceiverStatus::NotReady, _) | (_, ReceiverStatus::NotReady) => {
                // No more messages to process, so flush any messages buffered in the transport.
                ready!(self.transport().poll_flush(cx)?);

                // Even if we fully-flush, we return Pending, because we have no more requests
                // or cancellations right now.
                Poll::Pending
            }
        }
    }

    /// Yields the next pending request, if one is ready to be sent.
    fn poll_next_request(
        self: &mut PinMut<Self>,
        cx: &mut task::Context,
    ) -> Poll<Option<io::Result<DispatchRequest<Req, Resp>>>> {
        if self.in_flight_requests().len() >= self.config.max_in_flight_requests {
            info!(
                "At in-flight request capacity ({}/{}).",
                self.in_flight_requests().len(),
                self.config.max_in_flight_requests
            );

            // No need to schedule a wakeup, because timers and responses are responsible
            // for clearing out in-flight requests.
            return Poll::Pending;
        }

        while let Poll::Pending = self.transport().poll_ready(cx)? {
            // We can't yield a request-to-be-sent before the transport is capable of buffering it.
            ready!(self.transport().poll_flush(cx)?);
        }

        loop {
            match ready!(self.pending_requests().poll_next_unpin(cx)) {
                Some(request) => {
                    if request.response_completion.is_canceled() {
                        trace!(
                            "[{}] Request canceled before being sent.",
                            request.context.trace_id()
                        );
                        continue;
                    }

                    return Poll::Ready(Some(Ok(request)));
                }
                None => {
                    trace!("[{}] pending_requests closed", self.server_addr());
                    return Poll::Ready(None);
                }
            }
        }
    }

    /// Yields the next pending cancellation, and, if one is ready, cancels the associated request.
    fn poll_next_cancellation(
        self: &mut PinMut<Self>,
        cx: &mut task::Context,
    ) -> Poll<Option<io::Result<(client::Context, u64)>>> {
        while let Poll::Pending = self.transport().poll_ready(cx)? {
            ready!(self.transport().poll_flush(cx)?);
        }

        loop {
            match ready!(self.canceled_requests().poll_next_unpin(cx)) {
                Some(request_id) => {
                    if let Some(in_flight_data) = self.in_flight_requests().remove(&request_id) {
                        self.in_flight_requests().compact(0.1);

                        debug!(
                            "[{}/{}] Removed request.",
                            in_flight_data.context.trace_id(),
                            self.server_addr()
                        );

                        return Poll::Ready(Some(Ok((in_flight_data.context, request_id))));
                    }
                }
                None => {
                    trace!("[{}] canceled_requests closed.", self.server_addr());
                    return Poll::Ready(None);
                }
            }
        }
    }

    fn write_request(
        self: &mut PinMut<Self>,
        dispatch_request: DispatchRequest<Req, Resp>,
    ) -> io::Result<()> {
        let request_id = dispatch_request.request_id;
        let request = ClientMessage {
            trace_context: dispatch_request.context.trace_context,
            message: ClientMessageKind::Request(Request {
                id: request_id,
                message: dispatch_request.request,
                deadline: dispatch_request.context.deadline,
            }),
        };
        self.transport().start_send(request)?;
        self.in_flight_requests().insert(
            request_id,
            InFlightData {
                context: dispatch_request.context,
                response_completion: dispatch_request.response_completion,
            },
        );
        Ok(())
    }

    fn write_cancel(
        self: &mut PinMut<Self>,
        context: client::Context,
        request_id: u64,
    ) -> io::Result<()> {
        let trace_id = *context.trace_id();
        let cancel = ClientMessage {
            trace_context: context.trace_context,
            message: ClientMessageKind::Cancel { request_id },
        };
        self.transport().start_send(cancel)?;
        trace!("[{}/{}] Cancel message sent.", trace_id, self.server_addr());
        return Ok(());
    }

    /// Sends a server response to the client task that initiated the associated request.
    fn complete(self: &mut PinMut<Self>, response: Response<Resp>) -> bool {
        if let Some(in_flight_data) = self.in_flight_requests().remove(&response.request_id) {
            self.in_flight_requests().compact(0.1);

            trace!(
                "[{}/{}] Received response.",
                in_flight_data.context.trace_id(),
                self.server_addr()
            );
            let _ = in_flight_data.response_completion.send(response);
            return true;
        }

        debug!(
            "[{}] No in-flight request found for request_id = {}.",
            self.server_addr(),
            response.request_id
        );

        // If the response completion was absent, then the request was already canceled.
        false
    }
}

impl<Req, Resp, C> Future for RequestDispatch<Req, Resp, C>
where
    Req: Send,
    Resp: Send,
    C: Transport<Item = Response<Resp>, SinkItem = ClientMessage<Req>>,
{
    type Output = io::Result<()>;

    fn poll(mut self: PinMut<Self>, cx: &mut task::Context) -> Poll<io::Result<()>> {
        trace!("[{}] RequestDispatch::poll", self.server_addr());
        loop {
            match (self.pump_read(cx)?, self.pump_write(cx)?) {
                (read, write @ Poll::Ready(None)) => {
                    if self.in_flight_requests().is_empty() {
                        info!(
                            "[{}] Shutdown: write half closed, and no requests in flight.",
                            self.server_addr()
                        );
                        return Poll::Ready(Ok(()));
                    }
                    match read {
                        Poll::Ready(Some(())) => continue,
                        _ => {
                            trace!(
                                "[{}] read: {:?}, write: {:?}, (not ready)",
                                self.server_addr(),
                                read,
                                write,
                            );
                            return Poll::Pending;
                        }
                    }
                }
                (read @ Poll::Ready(Some(())), write) | (read, write @ Poll::Ready(Some(()))) => {
                    trace!(
                        "[{}] read: {:?}, write: {:?}",
                        self.server_addr(),
                        read,
                        write,
                    )
                }
                (read, write) => {
                    trace!(
                        "[{}] read: {:?}, write: {:?} (not ready)",
                        self.server_addr(),
                        read,
                        write,
                    );
                    return Poll::Pending;
                }
            }
        }
    }
}

/// A server-bound request sent from a [`Channel`] to request dispatch, which will then manage
/// the lifecycle of the request.
#[derive(Debug)]
struct DispatchRequest<Req, Resp> {
    context: client::Context,
    request_id: u64,
    request: Req,
    response_completion: oneshot::Sender<Response<Resp>>,
}

struct InFlightData<Resp> {
    context: client::Context,
    response_completion: oneshot::Sender<Response<Resp>>,
}

/// Sends request cancellation signals.
#[derive(Debug, Clone)]
struct RequestCancellation(mpsc::UnboundedSender<u64>);

/// A stream of IDs of requests that have been canceled.
#[derive(Debug)]
struct CanceledRequests(mpsc::UnboundedReceiver<u64>);

/// Returns a channel to send request cancellation messages.
fn cancellations() -> (RequestCancellation, CanceledRequests) {
    // Unbounded because messages are sent in the drop fn. This is fine, because it's still
    // bounded by the number of in-flight requests. Additionally, each request has a clone
    // of the sender, so the bounded channel would have the same behavior,
    // since it guarantees a slot.
    let (tx, rx) = mpsc::unbounded();
    (RequestCancellation(tx), CanceledRequests(rx))
}

impl RequestCancellation {
    /// Cancels the request with ID `request_id`.
    fn cancel(&mut self, request_id: u64) {
        let _ = self.0.unbounded_send(request_id);
    }
}

impl Stream for CanceledRequests {
    type Item = u64;

    fn poll_next(mut self: PinMut<Self>, cx: &mut task::Context) -> Poll<Option<u64>> {
        self.0.poll_next_unpin(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::{CanceledRequests, Channel, RequestCancellation, RequestDispatch};
    use crate::client::{self, Config};
    use crate::transport::{self, channel::UnboundedChannel};
    use crate::ClientMessage;
    use crate::Response;
    use env_logger;
    use fnv::FnvHashMap;
    use futures::{self, channel::mpsc, prelude::*};
    use futures_test::task::{noop_local_waker_ref, panic_context};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::pin::PinMut;

    #[test]
    fn stage_request() {
        let _ = env_logger::try_init();

        let (mut dispatch, mut channel, _server_channel) = test_dispatch();
        // Test that a request future dropped before it's processed by dispatch will cause the request
        // to not be added to the in-flight request map.
        let _resp = futures::executor::block_on(channel.start_send(
            client::Context::current(),
            0,
            "hi".to_string(),
        )).unwrap();

        let mut dispatch = PinMut::new(&mut dispatch);
        let mut cx = panic_context();
        let cx = &mut cx.with_waker(noop_local_waker_ref());

        let req = dispatch.poll_next_request(cx).ready();
        assert!(req.is_some());

        let req = req.unwrap();
        assert_eq!(req.request_id, 0);
        assert_eq!(req.request, "hi".to_string());
    }

    #[test]
    fn stage_request_response_future_dropped() {
        let _ = env_logger::try_init();

        let (mut dispatch, mut channel, _server_channel) = test_dispatch();
        // Test that a request future dropped before it's processed by dispatch will cause the request
        // to not be added to the in-flight request map.
        let resp = futures::executor::block_on(channel.start_send(
            client::Context::current(),
            0,
            "hi".into(),
        )).unwrap();
        drop(resp);
        drop(channel);

        let mut dispatch = PinMut::new(&mut dispatch);
        let mut cx = panic_context();
        let cx = &mut cx.with_waker(noop_local_waker_ref());

        dispatch.poll_next_cancellation(cx).unwrap();
        assert!(dispatch.poll_next_request(cx).ready().is_none());
    }

    #[test]
    fn stage_request_response_future_closed() {
        let _ = env_logger::try_init();

        let (mut dispatch, mut channel, _server_channel) = test_dispatch();

        // Test that a request future that's closed its receiver but not yet canceled its request --
        // i.e. still in `drop fn` -- will cause the request to not be added to the in-flight request
        // map.
        let resp = futures::executor::block_on(channel.start_send(
            client::Context::current(),
            1,
            "hi".into(),
        )).unwrap();
        drop(resp);
        drop(channel);

        let mut dispatch = PinMut::new(&mut dispatch);
        let mut cx = panic_context();
        let cx = &mut cx.with_waker(noop_local_waker_ref());
        assert!(dispatch.poll_next_request(cx).ready().is_none());
    }

    fn test_dispatch() -> (
        RequestDispatch<String, String, UnboundedChannel<Response<String>, ClientMessage<String>>>,
        Channel<String, String>,
        UnboundedChannel<ClientMessage<String>, Response<String>>,
    ) {
        let (to_dispatch, pending_requests) = mpsc::channel(1);
        let (cancel_tx, canceled_requests) = mpsc::unbounded();
        let (client_channel, server_channel) = transport::channel::unbounded();

        let dispatch = RequestDispatch::<String, String, _> {
            /// Writes requests to the wire and reads responses off the wire.
            transport: client_channel.fuse(),
            /// The next request to write to the wire.
            /// Requests waiting to be written to the wire.
            pending_requests: pending_requests.fuse(),
            /// Requests that were dropped.
            canceled_requests: CanceledRequests(canceled_requests),
            /// Requests already written to the wire that haven't yet received responses.
            in_flight_requests: FnvHashMap::default(),
            /// Configures limits to prevent unlimited resource usage.
            config: Config::default(),
            /// The address of the server connected to.
            server_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        };

        let cancellation = RequestCancellation(cancel_tx);
        let channel = Channel {
            to_dispatch,
            cancellation,
        };

        (dispatch, channel, server_channel)
    }

    trait PollTest {
        type T;
        fn unwrap(self) -> Poll<Self::T>;
        fn ready(self) -> Self::T;
    }

    impl<T, E> PollTest for Poll<Option<Result<T, E>>>
    where
        E: ::std::fmt::Display + Send + 'static,
    {
        type T = Option<T>;

        fn unwrap(self) -> Poll<Option<T>> {
            match self {
                Poll::Ready(Some(Ok(t))) => Poll::Ready(Some(t)),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Ready(Some(Err(e))) => panic!(e.to_string()),
                Poll::Pending => Poll::Pending,
            }
        }

        fn ready(self) -> Option<T> {
            match self {
                Poll::Ready(Some(Ok(t))) => Some(t),
                Poll::Ready(None) => None,
                Poll::Ready(Some(Err(e))) => panic!(e.to_string()),
                Poll::Pending => panic!("Pending"),
            }
        }
    }

}
