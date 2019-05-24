// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::{
    server::{self, Channel},
    util::Compact,
    PollIo, Response,
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
    collections::hash_map::Entry, fmt, hash::Hash, io, marker::Unpin, ops::Try, option::NoneError,
    pin::Pin,
};

/// Configures channel limits.
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// The maximum number of open channels the server will allow. When at the
    /// limit, existing channels are honored and new channels are rejected.
    pub channels: usize,
    /// The maximum number of channels per key. When a key is at the limit, existing channels are
    /// honored and new channels mapped to that key are rejected.
    pub channels_per_key: usize,
}

impl Limits {
    /// Set the maximum number of channels.
    pub fn channels(&mut self, n: usize) -> &mut Self {
        self.channels = n;
        self
    }

    /// Sets the maximum number of channels per key.
    pub fn channels_per_key(&mut self, n: usize) -> &mut Self {
        self.channels_per_key = n;
        self
    }
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            channels: 1_000_000,
            channels_per_key: 1_000,
        }
    }
}

/// Filters channels under configurable conditions:
///
/// 1. If the max number of channels is reached.
/// 2. If the max number of channels for a single key is reached.
#[derive(Debug)]
pub struct ChannelFilter<S, K, F>
where
    K: Eq + Hash,
{
    listener: Fuse<S>,
    closed_channels: mpsc::UnboundedSender<K>,
    closed_channels_rx: mpsc::UnboundedReceiver<K>,
    limits: Limits,
    key_counts: FnvHashMap<K, usize>,
    open_channels: usize,
    keymaker: F,
}

/// A channel that is tracked by a ChannelFilter.
#[derive(Debug)]
pub struct TrackedChannel<C, K>
where
    K: fmt::Display + Clone,
{
    inner: C,
    tracker: Tracker<K>,
}

impl<C, K> TrackedChannel<C, K>
where
    K: fmt::Display + Clone,
{
    unsafe_pinned!(inner: C);
}

#[derive(Debug)]
struct Tracker<K>
where
    K: fmt::Display + Clone,
{
    closed_channels: mpsc::UnboundedSender<K>,
    /// A key that uniquely identifies a device, such as an IP address.
    client_key: K,
}

impl<K> Drop for Tracker<K>
where
    K: fmt::Display + Clone,
{
    fn drop(&mut self) {
        trace!("[{}] Tracker dropped.", self.client_key);

        // Even in a bounded channel, each channel would have a guaranteed slot, so using
        // an unbounded sender is actually no different. And, the bound is on the maximum number
        // of open channels.
        if self
            .closed_channels
            .unbounded_send(self.client_key.clone())
            .is_err()
        {
            warn!(
                "[{}] Failed to send closed channel message.",
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

impl<C, K> Stream for TrackedChannel<C, K>
where
    C: Channel,
    K: fmt::Display + Clone + Send + 'static,
{
    type Item = <C as Stream>::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        self.transport().poll_next(cx)
    }
}

impl<C, K> Sink<Response<C::Resp>> for TrackedChannel<C, K>
where
    C: Channel,
    K: fmt::Display + Clone + Send + 'static,
{
    type SinkError = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::SinkError>> {
        self.transport().poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: Response<C::Resp>) -> Result<(), Self::SinkError> {
        self.transport().start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::SinkError>> {
        self.transport().poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::SinkError>> {
        self.transport().poll_close(cx)
    }
}

impl<C, K, T> AsRef<T> for TrackedChannel<C, K>
where
    C: AsRef<T>,
    K: fmt::Display + Clone + Send + 'static,
{
    fn as_ref(&self) -> &T {
        self.inner.as_ref()
    }
}

impl<C, K> Channel for TrackedChannel<C, K>
where
    C: Channel,
    K: fmt::Display + Clone + Send + 'static,
{
    type Req = C::Req;
    type Resp = C::Resp;

    fn config(&self) -> &server::Config {
        self.inner.config()
    }
}

impl<C, K> TrackedChannel<C, K>
where
    K: fmt::Display + Clone + Send + 'static,
{
    /// Returns the inner transport.
    pub fn get_ref(&self) -> &C {
        &self.inner
    }

    /// Returns the pinned inner transport.
    fn transport<'a>(self: Pin<&'a mut Self>) -> Pin<&'a mut C> {
        self.inner()
    }
}

enum NewChannel<C, K>
where
    K: fmt::Display + Clone,
{
    Filtered,
    Accepted(TrackedChannel<C, K>),
}

impl<C, K> Try for NewChannel<C, K>
where
    K: fmt::Display + Clone,
{
    type Ok = TrackedChannel<C, K>;
    type Error = NoneError;

    fn into_result(self) -> Result<TrackedChannel<C, K>, NoneError> {
        match self {
            NewChannel::Filtered => Err(NoneError),
            NewChannel::Accepted(channel) => Ok(channel),
        }
    }

    fn from_error(_: NoneError) -> Self {
        NewChannel::Filtered
    }

    fn from_ok(channel: TrackedChannel<C, K>) -> Self {
        NewChannel::Accepted(channel)
    }
}

impl<S, K, F> ChannelFilter<S, K, F>
where
    K: fmt::Display + Eq + Hash + Clone,
{
    unsafe_pinned!(open_channels: usize);
    unsafe_pinned!(limits: Limits);
    unsafe_pinned!(key_counts: FnvHashMap<K, usize>);
    unsafe_pinned!(closed_channels_rx: mpsc::UnboundedReceiver<K>);
    unsafe_pinned!(listener: Fuse<S>);
    unsafe_pinned!(keymaker: F);
}

impl<S, K, F> ChannelFilter<S, K, F>
where
    K: Eq + Hash,
    S: Stream,
{
    /// Sheds new channels to stay under configured limits.
    pub(crate) fn new(listener: S, limits: Limits, keymaker: F) -> Self
where {
        let (closed_channels, closed_channels_rx) = mpsc::unbounded();

        ChannelFilter {
            listener: listener.fuse(),
            closed_channels,
            closed_channels_rx,
            limits,
            key_counts: FnvHashMap::default(),
            open_channels: 0,
            keymaker,
        }
    }
}

impl<S, C, K, F> ChannelFilter<S, K, F>
where
    S: Stream<Item = io::Result<C>>,
    C: Channel,
    K: fmt::Display + Eq + Hash + Clone + Unpin,
    F: Fn(&C) -> K,
{
    fn handle_new_channel(self: &mut Pin<&mut Self>, stream: C) -> NewChannel<C, K> {
        let key = self.as_mut().keymaker()(&stream);
        let open_channels = *self.as_mut().open_channels();
        if open_channels >= self.as_mut().limits().channels {
            warn!(
                "[{}] Shedding channel because the maximum open channels \
                 limit is reached ({}/{}).",
                key,
                open_channels,
                self.as_mut().limits().channels
            );
            return NewChannel::Filtered;
        }

        let limits = self.limits.clone();
        let open_channels_for_ip = self.increment_channels_for_key(key.clone())?;
        *self.as_mut().open_channels() += 1;

        debug!(
            "[{}] Opening channel ({}/{} channels for IP, {} total).",
            key,
            open_channels_for_ip,
            limits.channels_per_key,
            self.as_mut().open_channels(),
        );

        NewChannel::Accepted(TrackedChannel {
            tracker: Tracker {
                client_key: key,
                closed_channels: self.closed_channels.clone(),
            },
            inner: stream,
        })
    }

    fn handle_closed_channel(self: &mut Pin<&mut Self>, key: K) {
        *self.as_mut().open_channels() -= 1;
        debug!(
            "[{}] Closed channel. {} open channels remaining.",
            key, self.open_channels
        );
        self.decrement_channels_for_key(key);
        self.as_mut().key_counts().compact(0.1);
    }

    fn increment_channels_for_key(self: &mut Pin<&mut Self>, key: K) -> Option<usize> {
        let channels_per_key = self.as_mut().limits().channels_per_key;
        let mut occupied;
        let mut key_counts = self.as_mut().key_counts();
        let occupied = match key_counts.entry(key.clone()) {
            Entry::Vacant(vacant) => vacant.insert(0),
            Entry::Occupied(o) => {
                if *o.get() < channels_per_key {
                    // Store the reference outside the block to extend the lifetime.
                    occupied = o;
                    occupied.get_mut()
                } else {
                    info!(
                        "[{}] Opened max channels from IP ({}/{}).",
                        key,
                        o.get(),
                        channels_per_key
                    );
                    return None;
                }
            }
        };
        *occupied += 1;
        Some(*occupied)
    }

    fn decrement_channels_for_key(self: &mut Pin<&mut Self>, key: K) {
        match self.as_mut().key_counts().entry(key.clone()) {
            Entry::Vacant(_) => {
                error!("[{}] Got vacant entry when closing channel.", key);
                return;
            }
            Entry::Occupied(mut occupied) => {
                *occupied.get_mut() -= 1;
                if *occupied.get() == 0 {
                    occupied.remove();
                    self.as_mut().key_counts().compact(0.1);
                }
            }
        };
    }

    fn poll_listener(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> PollIo<NewChannel<C, K>> {
        match ready!(self.as_mut().listener().poll_next_unpin(cx)?) {
            Some(codec) => Poll::Ready(Some(Ok(self.handle_new_channel(codec)))),
            None => Poll::Ready(None),
        }
    }

    fn poll_closed_channels(
        self: &mut Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        match ready!(self.as_mut().closed_channels_rx().poll_next_unpin(cx)) {
            Some(key) => {
                self.handle_closed_channel(key);
                Poll::Ready(Ok(()))
            }
            None => unreachable!("Holding a copy of closed_channels and didn't close it."),
        }
    }
}

impl<S, C, K, F> Stream for ChannelFilter<S, K, F>
where
    S: Stream<Item = io::Result<C>>,
    C: Channel,
    K: fmt::Display + Eq + Hash + Clone + Unpin,
    F: Fn(&C) -> K,
{
    type Item = io::Result<TrackedChannel<C, K>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> PollIo<TrackedChannel<C, K>> {
        loop {
            match (
                self.as_mut().poll_listener(cx)?,
                self.poll_closed_channels(cx)?,
            ) {
                (Poll::Ready(Some(NewChannel::Accepted(channel))), _) => {
                    return Poll::Ready(Some(Ok(channel)));
                }
                (Poll::Ready(Some(NewChannel::Filtered)), _) => {
                    trace!(
                        "Filtered a channel; {} open.",
                        self.as_mut().open_channels()
                    );
                    continue;
                }
                (_, Poll::Ready(())) => continue,
                (Poll::Pending, Poll::Pending) => return Poll::Pending,
                (Poll::Ready(None), Poll::Pending) => {
                    if *self.as_mut().open_channels() > 0 {
                        trace!(
                            "Listener closed; {} open channels.",
                            self.as_mut().open_channels()
                        );
                        return Poll::Pending;
                    }
                    trace!("Shutting down listener: all channels closed, and no more coming.");
                    return Poll::Ready(None);
                }
            }
        }
    }
}
