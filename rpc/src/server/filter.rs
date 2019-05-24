// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::{
    server::{self, Channel},
    util::Compact,
    Response,
};
use fnv::FnvHashMap;
use futures::{
    channel::mpsc,
    prelude::*,
    ready,
    stream::Fuse,
    task::{Context, Poll},
};
use log::{debug, info, trace};
use pin_utils::{unsafe_pinned, unsafe_unpinned};
use std::sync::{Arc, Weak};
use std::{
    collections::hash_map::Entry, convert::TryInto, fmt, hash::Hash, io, marker::Unpin, ops::Try,
    pin::Pin,
};

/// A single-threaded filter that drops channels based on per-key limits.
#[derive(Debug)]
pub struct ChannelFilter<S, K, F>
where
    K: Eq + Hash,
{
    listener: Fuse<S>,
    channels_per_key: u32,
    dropped_keys: mpsc::UnboundedReceiver<K>,
    dropped_keys_tx: mpsc::UnboundedSender<K>,
    key_counts: FnvHashMap<K, Weak<Tracker<K>>>,
    keymaker: F,
}

/// A channel that is tracked by a ChannelFilter.
#[derive(Debug)]
pub struct TrackedChannel<C, K> {
    inner: C,
    tracker: Arc<Tracker<K>>,
}

impl<C, K> TrackedChannel<C, K> {
    unsafe_pinned!(inner: C);
}

#[derive(Debug)]
struct Tracker<K> {
    key: Option<K>,
    dropped_keys: mpsc::UnboundedSender<K>,
}

impl<K> Drop for Tracker<K> {
    fn drop(&mut self) {
        // Don't care if the listener is dropped.
        let _ = self.dropped_keys.unbounded_send(self.key.take().unwrap());
    }
}

/// A running handler serving all requests for a single client.
#[derive(Debug)]
pub struct TrackedHandler<K, Fut> {
    inner: Fut,
    tracker: Tracker<K>,
}

impl<K, Fut> TrackedHandler<K, Fut>
where
    Fut: Future,
{
    unsafe_pinned!(inner: Fut);
}

impl<K, Fut> Future for TrackedHandler<K, Fut>
where
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
{
    type Item = <C as Stream>::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        self.transport().poll_next(cx)
    }
}

impl<C, K> Sink<Response<C::Resp>> for TrackedChannel<C, K>
where
    C: Channel,
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

impl<C, T, K> AsRef<T> for TrackedChannel<C, K>
where
    C: AsRef<T>,
{
    fn as_ref(&self) -> &T {
        self.inner.as_ref()
    }
}

impl<C, K> Channel for TrackedChannel<C, K>
where
    C: Channel,
{
    type Req = C::Req;
    type Resp = C::Resp;

    fn config(&self) -> &server::Config {
        self.inner.config()
    }
}

impl<C, K> TrackedChannel<C, K> {
    /// Returns the inner transport.
    pub fn get_ref(&self) -> &C {
        &self.inner
    }

    /// Returns the pinned inner transport.
    fn transport<'a>(self: Pin<&'a mut Self>) -> Pin<&'a mut C> {
        self.inner()
    }
}

enum NewChannel<C, K> {
    Accepted(TrackedChannel<C, K>),
    Filtered(K),
}

impl<C, K> Try for NewChannel<C, K> {
    type Ok = TrackedChannel<C, K>;
    type Error = K;

    fn into_result(self) -> Result<TrackedChannel<C, K>, K> {
        match self {
            NewChannel::Accepted(channel) => Ok(channel),
            NewChannel::Filtered(k) => Err(k),
        }
    }

    fn from_error(k: K) -> Self {
        NewChannel::Filtered(k)
    }

    fn from_ok(channel: TrackedChannel<C, K>) -> Self {
        NewChannel::Accepted(channel)
    }
}

impl<S, K, F> ChannelFilter<S, K, F>
where
    K: fmt::Display + Eq + Hash + Clone,
{
    unsafe_pinned!(listener: Fuse<S>);
    unsafe_pinned!(dropped_keys: mpsc::UnboundedReceiver<K>);
    unsafe_pinned!(dropped_keys_tx: mpsc::UnboundedSender<K>);
    unsafe_unpinned!(key_counts: FnvHashMap<K, Weak<Tracker<K>>>);
    unsafe_unpinned!(channels_per_key: u32);
    unsafe_unpinned!(keymaker: F);
}

impl<S, K, F> ChannelFilter<S, K, F>
where
    K: Eq + Hash,
    S: Stream,
{
    /// Sheds new channels to stay under configured limits.
    pub(crate) fn new(listener: S, channels_per_key: u32, keymaker: F) -> Self {
        let (dropped_keys_tx, dropped_keys) = mpsc::unbounded();
        ChannelFilter {
            listener: listener.fuse(),
            channels_per_key,
            dropped_keys,
            dropped_keys_tx,
            key_counts: FnvHashMap::default(),
            keymaker,
        }
    }
}

impl<S, C, K, F> ChannelFilter<S, K, F>
where
    S: Stream<Item = C>,
    C: Channel,
    K: fmt::Display + Eq + Hash + Clone + Unpin,
    F: Fn(&C) -> K,
{
    fn handle_new_channel(self: &mut Pin<&mut Self>, stream: C) -> NewChannel<C, K> {
        let key = self.as_mut().keymaker()(&stream);
        let tracker = self.increment_channels_for_key(key.clone())?;
        let max = self.as_mut().channels_per_key();

        debug!(
            "[{}] Opening channel ({}/{}) channels for key.",
            key,
            Arc::strong_count(&tracker),
            max
        );

        NewChannel::Accepted(TrackedChannel {
            tracker,
            inner: stream,
        })
    }

    fn increment_channels_for_key(self: &mut Pin<&mut Self>, key: K) -> Result<Arc<Tracker<K>>, K> {
        let channels_per_key = self.channels_per_key;
        let dropped_keys = self.dropped_keys_tx.clone();
        let key_counts = &mut self.as_mut().key_counts();
        match key_counts.entry(key.clone()) {
            Entry::Vacant(vacant) => {
                let tracker = Arc::new(Tracker {
                    key: Some(key),
                    dropped_keys,
                });

                vacant.insert(Arc::downgrade(&tracker));
                Ok(tracker)
            }
            Entry::Occupied(mut o) => {
                let count = o.get().strong_count();
                if count >= channels_per_key.try_into().unwrap() {
                    info!(
                        "[{}] Opened max channels from key ({}/{}).",
                        key, count, channels_per_key
                    );
                    Err(key)
                } else {
                    Ok(o.get().upgrade().unwrap_or_else(|| {
                        let tracker = Arc::new(Tracker {
                            key: Some(key),
                            dropped_keys,
                        });

                        *o.get_mut() = Arc::downgrade(&tracker);
                        tracker
                    }))
                }
            }
        }
    }

    fn poll_listener(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<NewChannel<C, K>>> {
        match ready!(self.as_mut().listener().poll_next_unpin(cx)) {
            Some(codec) => Poll::Ready(Some(self.handle_new_channel(codec))),
            None => Poll::Ready(None),
        }
    }

    fn poll_closed_channels(self: &mut Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        match ready!(self.as_mut().dropped_keys().poll_next_unpin(cx)) {
            Some(key) => {
                debug!("All channels dropped for key [{}]", key);
                self.as_mut().key_counts().remove(&key);
                self.as_mut().key_counts().compact(0.1);
                Poll::Ready(())
            }
            None => unreachable!("Holding a copy of closed_channels and didn't close it."),
        }
    }
}

impl<S, C, K, F> Stream for ChannelFilter<S, K, F>
where
    S: Stream<Item = C>,
    C: Channel,
    K: fmt::Display + Eq + Hash + Clone + Unpin,
    F: Fn(&C) -> K,
{
    type Item = TrackedChannel<C, K>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<TrackedChannel<C, K>>> {
        loop {
            match (
                self.as_mut().poll_listener(cx),
                self.poll_closed_channels(cx),
            ) {
                (Poll::Ready(Some(NewChannel::Accepted(channel))), _) => {
                    return Poll::Ready(Some(channel));
                }
                (Poll::Ready(Some(NewChannel::Filtered(_))), _) => {
                    continue;
                }
                (_, Poll::Ready(())) => continue,
                (Poll::Pending, Poll::Pending) => return Poll::Pending,
                (Poll::Ready(None), Poll::Pending) => {
                    trace!("Shutting down listener.");
                    return Poll::Ready(None);
                }
            }
        }
    }
}
