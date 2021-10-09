use super::{
    limits::{channels_per_key::MaxChannelsPerKey, requests_per_channel::MaxRequestsPerChannel},
    Channel,
};
use futures::prelude::*;
use std::{fmt, hash::Hash};

#[cfg(feature = "tokio1")]
use super::{tokio::TokioServerExecutor, Serve};

/// An extension trait for [streams](futures::prelude::Stream) of [`Channels`](Channel).
pub trait Incoming<C>
where
    Self: Sized + Stream<Item = C>,
    C: Channel,
{
    /// Enforces channel per-key limits.
    fn max_channels_per_key<K, KF>(self, n: u32, keymaker: KF) -> MaxChannelsPerKey<Self, K, KF>
    where
        K: fmt::Display + Eq + Hash + Clone + Unpin,
        KF: Fn(&C) -> K,
    {
        MaxChannelsPerKey::new(self, n, keymaker)
    }

    /// Caps the number of concurrent requests per channel.
    fn max_concurrent_requests_per_channel(self, n: usize) -> MaxRequestsPerChannel<Self> {
        MaxRequestsPerChannel::new(self, n)
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
        TokioServerExecutor::new(self, serve)
    }
}

impl<S, C> Incoming<C> for S
where
    S: Sized + Stream<Item = C>,
    C: Channel,
{
}
