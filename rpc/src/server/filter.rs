// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::{
    server::{self, Channel, BaseChannel},
    util::Compact,
    ClientMessage, PollIo, Response, Transport,
};
use fnv::FnvHashMap;
use futures::{
    channel::mpsc,
    prelude::*,
    ready,
    stream::Fuse,
    task::{Context, Poll},
};
use log::{debug, error, info, trace, warn};
use pin_utils::unsafe_pinned;
use std::{
    collections::hash_map::Entry,
    fmt,
    hash::Hash,
    io,
    marker::{PhantomData, Unpin},
    ops::Try,
    option::NoneError,
    pin::Pin,
};

/// Configures connection limits.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct Config {
    /// The maximum number of clients that can be connected to the server at once. When at the
    /// limit, existing connections are honored and new connections are rejected.
    pub max_connections: usize,
    /// The maximum number of clients per key that can be connected to the server at once. When a
    /// key is at the limit, existing connections are honored and new connections mapped to that
    /// key are rejected.
    pub max_connections_per_key: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_connections: 1_000_000,
            max_connections_per_key: 1_000,
        }
    }
}

/// Drops connections under configurable conditions:
///
/// 1. If the max number of connections is reached.
/// 2. If the max number of connections for a single IP is reached.
#[derive(Debug)]
pub struct ConnectionFilter<S, Req, Resp, K, F>
where
    K: Eq + Hash,
{
    listener: Fuse<S>,
    closed_connections: mpsc::UnboundedSender<K>,
    closed_connections_rx: mpsc::UnboundedReceiver<K>,
    config: Config,
    connections_per_key: FnvHashMap<K, usize>,
    open_connections: usize,
    keymaker: F,
    ghost: PhantomData<(Req, Resp)>,
}

/// A channel that is tracked by a ConnectionFilter.
#[derive(Debug)]
pub struct TrackedBaseChannel<Req, Resp, T, K>
where
    K: fmt::Display + Clone,
{
    inner: BaseChannel<Req, Resp, T>,
    tracker: Tracker<K>,
}

impl<Req, Resp, T, K> TrackedBaseChannel<Req, Resp, T, K>
where
    K: fmt::Display + Clone,
{
    unsafe_pinned!(inner: BaseChannel<Req, Resp, T>);
}

#[derive(Debug)]
struct Tracker<K>
where
    K: fmt::Display + Clone,
{
    closed_connections: mpsc::UnboundedSender<K>,
    /// A key that uniquely identifies a device, such as an IP address.
    client_key: K,
}

impl<K> Drop for Tracker<K>
where
    K: fmt::Display + Clone,
{
    fn drop(&mut self) {
        trace!("[{}] Closing channel.", self.client_key);

        // Even in a bounded channel, each connection would have a guaranteed slot, so using
        // an unbounded sender is actually no different. And, the bound is on the maximum number
        // of open connections.
        if self
            .closed_connections
            .unbounded_send(self.client_key.clone())
            .is_err()
        {
            warn!(
                "[{}] Failed to send closed connection message.",
                self.client_key
            );
        }
    }
}

/// A running handler serving all requests for a single client.
#[derive(Debug)]
pub struct TrackedHandler<Fut, K>
where
    K: fmt::Display + Clone,
{
    inner: Fut,
    tracker: Tracker<K>,
}

impl<Fut, K> TrackedHandler<Fut, K>
where
    K: fmt::Display + Clone,
    Fut: Future,
{
    unsafe_pinned!(inner: Fut);
}

impl<Fut, K> Future for TrackedHandler<Fut, K>
where
    K: fmt::Display + Clone,
    Fut: Future,
{
    type Output = Fut::Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner().poll(cx)
    }
}

impl<Req, Resp, T, K> Channel for TrackedBaseChannel<Req, Resp, T, K>
where
    T: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send + 'static,
    Req: Send + 'static,
    Resp: Send + 'static,
    K: fmt::Display + Clone + Send + 'static,
{
    type Req = Req;
    type Resp = Resp;
    type T = T;

    fn transport<'a>(self: Pin<&'a mut Self>) -> Pin<&'a mut T> {
        Channel::transport(self.inner())
    }

    fn config(&self) -> &server::Config {
        &self.inner.config
    }

    fn get_ref(&self) -> &T {
        self.inner.get_ref()
    }
}

enum NewConnection<Req, Resp, T, K>
where
    K: fmt::Display + Clone,
{
    Filtered,
    Accepted(TrackedBaseChannel<Req, Resp, T, K>),
}

impl<Req, Resp, T, K> Try for NewConnection<Req, Resp, T, K>
where
    K: fmt::Display + Clone,
{
    type Ok = TrackedBaseChannel<Req, Resp, T, K>;
    type Error = NoneError;

    fn into_result(self) -> Result<TrackedBaseChannel<Req, Resp, T, K>, NoneError> {
        match self {
            NewConnection::Filtered => Err(NoneError),
            NewConnection::Accepted(channel) => Ok(channel),
        }
    }

    fn from_error(_: NoneError) -> Self {
        NewConnection::Filtered
    }

    fn from_ok(channel: TrackedBaseChannel<Req, Resp, T, K>) -> Self {
        NewConnection::Accepted(channel)
    }
}

impl<S, Req, Resp, K, F> ConnectionFilter<S, Req, Resp, K, F>
where
    K: fmt::Display + Eq + Hash + Clone,
{
    unsafe_pinned!(open_connections: usize);
    unsafe_pinned!(config: Config);
    unsafe_pinned!(connections_per_key: FnvHashMap<K, usize>);
    unsafe_pinned!(closed_connections_rx: mpsc::UnboundedReceiver<K>);
    unsafe_pinned!(listener: Fuse<S>);
    unsafe_pinned!(keymaker: F);
}

impl<S, Req, Resp, K, F> ConnectionFilter<S, Req, Resp, K, F>
where
    K: Eq + Hash,
    S: Stream,
{
    /// Sheds new connections to stay under configured limits.
    pub(crate) fn new(listener: S, config: Config, keymaker: F) -> Self
where {
        let (closed_connections, closed_connections_rx) = mpsc::unbounded();

        ConnectionFilter {
            listener: listener.fuse(),
            closed_connections,
            closed_connections_rx,
            config,
            connections_per_key: FnvHashMap::default(),
            open_connections: 0,
            keymaker,
            ghost: PhantomData,
        }
    }
}

impl<S, Req, Resp, K, F, T> ConnectionFilter<S, Req, Resp, K, F>
where
    S: Stream<Item = io::Result<BaseChannel<Req, Resp, T>>>,
    T: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send,
    K: fmt::Display + Eq + Hash + Clone + Unpin,
    F: Fn(&T) -> K,
{
    fn handle_new_connection(
        self: &mut Pin<&mut Self>,
        stream: BaseChannel<Req, Resp, T>,
    ) -> NewConnection<Req, Resp, T, K> {
        let key = self.as_mut().keymaker()(stream.get_ref());
        let open_connections = *self.as_mut().open_connections();
        if open_connections >= self.as_mut().config().max_connections {
            warn!(
                "[{}] Shedding connection because the maximum open connections \
                 limit is reached ({}/{}).",
                key,
                open_connections,
                self.as_mut().config().max_connections
            );
            return NewConnection::Filtered;
        }

        let config = self.config.clone();
        let open_connections_for_ip = self.increment_connections_for_key(key.clone())?;
        *self.as_mut().open_connections() += 1;

        debug!(
            "[{}] Opening channel ({}/{} connections for IP, {} total).",
            key,
            open_connections_for_ip,
            config.max_connections_per_key,
            self.as_mut().open_connections(),
        );

        NewConnection::Accepted(TrackedBaseChannel {
            tracker: Tracker {
                client_key: key,
                closed_connections: self.closed_connections.clone(),
            },
            inner: stream,
        })
    }

    fn handle_closed_connection(self: &mut Pin<&mut Self>, key: K) {
        *self.as_mut().open_connections() -= 1;
        debug!(
            "[{}] Closing channel. {} open connections remaining.",
            key, self.open_connections
        );
        self.decrement_connections_for_key(key);
        self.as_mut().connections_per_key().compact(0.1);
    }

    fn increment_connections_for_key(self: &mut Pin<&mut Self>, key: K) -> Option<usize> {
        let max_connections_per_key = self.as_mut().config().max_connections_per_key;
        let mut occupied;
        let mut connections_per_key = self.as_mut().connections_per_key();
        let occupied = match connections_per_key.entry(key.clone()) {
            Entry::Vacant(vacant) => vacant.insert(0),
            Entry::Occupied(o) => {
                if *o.get() < max_connections_per_key {
                    // Store the reference outside the block to extend the lifetime.
                    occupied = o;
                    occupied.get_mut()
                } else {
                    info!(
                        "[{}] Opened max connections from IP ({}/{}).",
                        key,
                        o.get(),
                        max_connections_per_key
                    );
                    return None;
                }
            }
        };
        *occupied += 1;
        Some(*occupied)
    }

    fn decrement_connections_for_key(self: &mut Pin<&mut Self>, key: K) {
        let should_compact = match self.as_mut().connections_per_key().entry(key.clone()) {
            Entry::Vacant(_) => {
                error!("[{}] Got vacant entry when closing connection.", key);
                return;
            }
            Entry::Occupied(mut occupied) => {
                *occupied.get_mut() -= 1;
                if *occupied.get() == 0 {
                    occupied.remove();
                    true
                } else {
                    false
                }
            }
        };
        if should_compact {
            self.as_mut().connections_per_key().compact(0.1);
        }
    }

    fn poll_listener(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> PollIo<NewConnection<Req, Resp, T, K>> {
        match ready!(self.as_mut().listener().poll_next_unpin(cx)?) {
            Some(codec) => Poll::Ready(Some(Ok(self.handle_new_connection(codec)))),
            None => Poll::Ready(None),
        }
    }

    fn poll_closed_connections(
        self: &mut Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        match ready!(self.as_mut().closed_connections_rx().poll_next_unpin(cx)) {
            Some(key) => {
                self.handle_closed_connection(key);
                Poll::Ready(Ok(()))
            }
            None => unreachable!("Holding a copy of closed_connections and didn't close it."),
        }
    }
}

impl<S, Req, Resp, T, K, F> Stream for ConnectionFilter<S, Req, Resp, K, F>
where
    S: Stream<Item = io::Result<BaseChannel<Req, Resp, T>>>,
    T: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send,
    K: fmt::Display + Eq + Hash + Clone + Unpin,
    F: Fn(&T) -> K,
{
    type Item = io::Result<TrackedBaseChannel<Req, Resp, T, K>>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> PollIo<TrackedBaseChannel<Req, Resp, T, K>> {
        loop {
            match (
                self.as_mut().poll_listener(cx)?,
                self.poll_closed_connections(cx)?,
            ) {
                (Poll::Ready(Some(NewConnection::Accepted(channel))), _) => {
                    return Poll::Ready(Some(Ok(channel)));
                }
                (Poll::Ready(Some(NewConnection::Filtered)), _) | (_, Poll::Ready(())) => {
                    trace!(
                        "Filtered a connection; {} open.",
                        self.as_mut().open_connections()
                    );
                    continue;
                }
                (Poll::Pending, Poll::Pending) => return Poll::Pending,
                (Poll::Ready(None), Poll::Pending) => {
                    if *self.as_mut().open_connections() > 0 {
                        trace!(
                            "Listener closed; {} open connections.",
                            self.as_mut().open_connections()
                        );
                        return Poll::Pending;
                    }
                    trace!("Shutting down listener: all connections closed, and no more coming.");
                    return Poll::Ready(None);
                }
            }
        }
    }
}
