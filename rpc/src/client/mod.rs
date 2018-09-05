//! Provides a client that connects to a server and sends multiplexed requests.

use crate::util::{deadline_compat, AsDuration};
use crate::Response;
use crate::Transport;
use humantime::{format_duration, format_rfc3339};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::atomic::AtomicU64,
};

use std::{
    sync::{atomic::Ordering, Arc},
    time::Instant,
};

use crate::context::Context;
use crate::ClientMessage;

mod dispatch;

/// Sends multiplexed requests to, and receives responses from, a server.
#[derive(Debug)]
pub struct Client<Req, Resp> {
    /// Channel to send requests to the dispatch task.
    channel: dispatch::Channel<Req, Resp>,
    /// The ID to use for the next request to stage.
    next_request_id: Arc<AtomicU64>,
    server_addr: SocketAddr,
}

impl<Req, Resp> Clone for Client<Req, Resp> {
    fn clone(&self) -> Self {
        Client {
            channel: self.channel.clone(),
            next_request_id: self.next_request_id.clone(),
            server_addr: self.server_addr,
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
    pub async fn new<T>(config: Config, transport: T) -> Self
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

        Client {
            channel: await!(dispatch::spawn(config, transport, server_addr)),
            server_addr,
            next_request_id: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Initiates a request, sending it to the dispatch task.
    ///
    /// Returns a [`Future`] that resolves to this client and the future response
    /// once the request is successfully enqueued.
    ///
    /// [`Future`]: futures::Future
    pub async fn send<R>(&mut self, ctx: Context, request: R) -> io::Result<Resp>
    where
        Req: From<R>,
        R: 'static,
    {
        let request = Req::from(request);
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let timeout = ctx.deadline.as_duration();
        let deadline = Instant::now() + timeout;
        trace!(
            "[{}/{}] Queuing request with deadline {} (timeout {}).",
            ctx.trace_id(),
            self.server_addr,
            format_rfc3339(ctx.deadline),
            format_duration(timeout),
        );

        let server_addr = self.server_addr;

        let trace_id = *ctx.trace_id();
        let response = self.channel.send(ctx, request_id, request);
        let response = await!(deadline_compat::Deadline::new(response, deadline))
            .map_err(|e| {
                if e.is_elapsed() {
                    return io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Client dropped expired request.".to_string(),
                    );
                }

                if e.is_timer() {
                    let e = e.into_timer().unwrap();
                    return if e.is_at_capacity() {
                        io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Cancelling request because an expiration could not be set due to the timer \
                     being at capacity."
                                .to_string(),
                        )
                    } else if e.is_shutdown() {
                        panic!(
                            "[{}/{}] Timer was shutdown",
                            trace_id, server_addr
                        );
                    } else {
                        panic!(
                            "[{}/{}] Unrecognized timer error: {}",
                            trace_id, server_addr, e
                        )
                    };
                }

                if e.is_inner() {
                    return e.into_inner().unwrap();
                }

                panic!(
                    "[{}/{}] Unrecognized deadline error: {}",
                    trace_id, server_addr, e
                );
            }
        )?;
        Ok(response.message?)
    }
}
