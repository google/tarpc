// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::{
    server::{Channel, Config},
    util::Compact,
    ClientMessage, Response, Transport,
};
use fnv::FnvHashMap;
use futures::{channel::mpsc, prelude::*, ready, stream::Fuse, task::{LocalWaker, Poll}};
use log::{debug, error, info, trace, warn};
use pin_utils::unsafe_pinned;
use std::{
    collections::hash_map::Entry,
    io,
    marker::PhantomData,
    net::{IpAddr, SocketAddr},
    ops::Try,
    option::NoneError,
    pin::Pin,
};

/// Drops connections under configurable conditions:
///
/// 1. If the max number of connections is reached.
/// 2. If the max number of connections for a single IP is reached.
#[derive(Debug)]
pub struct ConnectionFilter<S, Req, Resp> {
    listener: Fuse<S>,
    closed_connections: mpsc::UnboundedSender<SocketAddr>,
    closed_connections_rx: mpsc::UnboundedReceiver<SocketAddr>,
    config: Config,
    connections_per_ip: FnvHashMap<IpAddr, usize>,
    open_connections: usize,
    ghost: PhantomData<(Req, Resp)>,
}

enum NewConnection<Req, Resp, C> {
    Filtered,
    Accepted(Channel<Req, Resp, C>),
}

impl<Req, Resp, C> Try for NewConnection<Req, Resp, C> {
    type Ok = Channel<Req, Resp, C>;
    type Error = NoneError;

    fn into_result(self) -> Result<Channel<Req, Resp, C>, NoneError> {
        match self {
            NewConnection::Filtered => Err(NoneError),
            NewConnection::Accepted(channel) => Ok(channel),
        }
    }

    fn from_error(_: NoneError) -> Self {
        NewConnection::Filtered
    }

    fn from_ok(channel: Channel<Req, Resp, C>) -> Self {
        NewConnection::Accepted(channel)
    }
}

impl<S, Req, Resp> ConnectionFilter<S, Req, Resp> {
    unsafe_pinned!(open_connections: usize);
    unsafe_pinned!(config: Config);
    unsafe_pinned!(connections_per_ip: FnvHashMap<IpAddr, usize>);
    unsafe_pinned!(closed_connections_rx: mpsc::UnboundedReceiver<SocketAddr>);
    unsafe_pinned!(listener: Fuse<S>);

    /// Sheds new connections to stay under configured limits.
    pub fn filter<C>(listener: S, config: Config) -> Self
    where
        S: Stream<Item = Result<C, io::Error>>,
        C: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send,
    {
        let (closed_connections, closed_connections_rx) = mpsc::unbounded();

        ConnectionFilter {
            listener: listener.fuse(),
            closed_connections,
            closed_connections_rx,
            config,
            connections_per_ip: FnvHashMap::default(),
            open_connections: 0,
            ghost: PhantomData,
        }
    }

    fn handle_new_connection<C>(self: &mut Pin<&mut Self>, stream: C) -> NewConnection<Req, Resp, C>
    where
        C: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send,
    {
        let peer = match stream.peer_addr() {
            Ok(peer) => peer,
            Err(e) => {
                warn!("Could not get peer_addr of new connection: {}", e);
                return NewConnection::Filtered;
            }
        };

        let open_connections = *self.open_connections();
        if open_connections >= self.config().max_connections {
            warn!(
                "[{}] Shedding connection because the maximum open connections \
                 limit is reached ({}/{}).",
                peer,
                open_connections,
                self.config().max_connections
            );
            return NewConnection::Filtered;
        }

        let config = self.config.clone();
        let open_connections_for_ip = self.increment_connections_for_ip(&peer)?;
        *self.open_connections() += 1;

        debug!(
            "[{}] Opening channel ({}/{} connections for IP, {} total).",
            peer,
            open_connections_for_ip,
            config.max_connections_per_ip,
            self.open_connections(),
        );

        NewConnection::Accepted(Channel {
            client_addr: peer,
            closed_connections: self.closed_connections.clone(),
            transport: stream.fuse(),
            config,
            ghost: PhantomData,
        })
    }

    fn handle_closed_connection(self: &mut Pin<&mut Self>, addr: &SocketAddr) {
        *self.open_connections() -= 1;
        debug!(
            "[{}] Closing channel. {} open connections remaining.",
            addr, self.open_connections
        );
        self.decrement_connections_for_ip(&addr);
        self.connections_per_ip().compact(0.1);
    }

    fn increment_connections_for_ip(self: &mut Pin<&mut Self>, peer: &SocketAddr) -> Option<usize> {
        let max_connections_per_ip = self.config().max_connections_per_ip;
        let mut occupied;
        let mut connections_per_ip = self.connections_per_ip();
        let occupied = match connections_per_ip.entry(peer.ip()) {
            Entry::Vacant(vacant) => vacant.insert(0),
            Entry::Occupied(o) => {
                if *o.get() < max_connections_per_ip {
                    // Store the reference outside the block to extend the lifetime.
                    occupied = o;
                    occupied.get_mut()
                } else {
                    info!(
                        "[{}] Opened max connections from IP ({}/{}).",
                        peer,
                        o.get(),
                        max_connections_per_ip
                    );
                    return None;
                }
            }
        };
        *occupied += 1;
        Some(*occupied)
    }

    fn decrement_connections_for_ip(self: &mut Pin<&mut Self>, addr: &SocketAddr) {
        let should_compact = match self.connections_per_ip().entry(addr.ip()) {
            Entry::Vacant(_) => {
                error!("[{}] Got vacant entry when closing connection.", addr);
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
            self.connections_per_ip().compact(0.1);
        }
    }

    fn poll_listener<C>(
        self: &mut Pin<&mut Self>,
        cx: &LocalWaker,
    ) -> Poll<Option<io::Result<NewConnection<Req, Resp, C>>>>
    where
        S: Stream<Item = Result<C, io::Error>>,
        C: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send,
    {
        match ready!(self.listener().poll_next_unpin(cx)?) {
            Some(codec) => Poll::Ready(Some(Ok(self.handle_new_connection(codec)))),
            None => Poll::Ready(None),
        }
    }

    fn poll_closed_connections(
        self: &mut Pin<&mut Self>,
        cx: &LocalWaker,
    ) -> Poll<io::Result<()>> {
        match ready!(self.closed_connections_rx().poll_next_unpin(cx)) {
            Some(addr) => {
                self.handle_closed_connection(&addr);
                Poll::Ready(Ok(()))
            }
            None => unreachable!("Holding a copy of closed_connections and didn't close it."),
        }
    }
}

impl<S, Req, Resp, T> Stream for ConnectionFilter<S, Req, Resp>
where
    S: Stream<Item = Result<T, io::Error>>,
    T: Transport<Item = ClientMessage<Req>, SinkItem = Response<Resp>> + Send,
{
    type Item = io::Result<Channel<Req, Resp, T>>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &LocalWaker,
    ) -> Poll<Option<io::Result<Channel<Req, Resp, T>>>> {
        loop {
            match (self.poll_listener(cx)?, self.poll_closed_connections(cx)?) {
                (Poll::Ready(Some(NewConnection::Accepted(channel))), _) => {
                    return Poll::Ready(Some(Ok(channel)))
                }
                (Poll::Ready(Some(NewConnection::Filtered)), _) | (_, Poll::Ready(())) => {
                    trace!("Filtered a connection; {} open.", self.open_connections());
                    continue;
                }
                (Poll::Pending, Poll::Pending) => return Poll::Pending,
                (Poll::Ready(None), Poll::Pending) => {
                    if *self.open_connections() > 0 {
                        trace!(
                            "Listener closed; {} open connections.",
                            self.open_connections()
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
