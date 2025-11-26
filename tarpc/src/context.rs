// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a request context that carries a deadline and trace context. This context is sent from
//! client to server and is used by the server to enforce response deadlines.

use crate::trace::{self, TraceId};
use opentelemetry::trace::TraceContextExt;
use static_assertions::assert_impl_all;
use std::ops::{Deref, DerefMut};
use std::{
    convert::TryFrom,
    time::{Duration, Instant},
};
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// A request context that carries request-scoped information like deadlines and trace information.
/// It is sent from client to server and is used by the server to enforce response deadlines.
///
/// The context should not be stored directly in a server implementation, because the context will
/// be different for each request in scope.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
pub struct SharedContext {
    /// When the client expects the request to be complete by. The server should cancel the request
    /// if it is not complete by this time.
    #[cfg_attr(feature = "serde1", serde(default = "ten_seconds_from_now"))]
    // Serialized as a Duration to prevent clock skew issues.
    #[cfg_attr(feature = "serde1", serde(with = "absolute_to_relative_time"))]
    pub deadline: Instant,
    /// Uniquely identifies requests originating from the same source.
    /// When a service handles a request by making requests itself, those requests should
    /// include the same `trace_id` as that included on the original request. This way,
    /// users can trace related actions across a distributed system.
    pub trace_context: trace::Context
}

///TODO
pub trait ExtractContext<Ctx> {
    ///TODO
    fn extract(&self) -> Ctx;
    ///TODO
    fn update(&mut self, value: Ctx);
}

impl<T> ExtractContext<T> for T where T: Clone {
    fn extract(&self) -> T {
        self.clone()
    }

    fn update(&mut self, value: T) {
        *self = value
    }
}

/// Request context that carries request-scoped client side information like deadlines and trace information
/// as well as any server side extensions defined by the transport, hooks and stubs.
/// The shared part of the context is sent from client to server, while the client side extensions are only seen on the client side.
///
/// The context should not be stored directly in a stub implementation, because the context will
/// be different for each request in scope.
#[derive(Debug)]
pub struct ClientContext {
    /// Shared context sent from client to server which contains information used by both sides.
    pub shared_context: SharedContext,

    /// Client side extensions that are not seen by the server
    /// XXX, YYY, and ZZZ can use this to store per-request data, and communicate with eachother.
    /// Note that this is NOT sent to the server, and they will always see an empty map here.
    pub client_context: anymap3::Map<dyn core::any::Any + Send + Sync>,
}

impl ClientContext {
    /// Creates a new ServerContext from the given SharedContext with no extensions.
    pub fn new(shared_context: SharedContext) -> Self {
        Self {
            shared_context,
            client_context: anymap3::Map::new(),
        }
    }

    /// Creates a new ClientContext for the current shared context with no extensions.
    pub fn current() -> Self {
        Self::new(SharedContext::current())
    }
}

impl ExtractContext<SharedContext> for ClientContext {
    fn extract(&self) -> SharedContext {
        self.shared_context.clone()
    }

    fn update(&mut self, value: SharedContext) {
        self.shared_context = value
    }
}

impl Deref for ClientContext {
    type Target = SharedContext;

    fn deref(&self) -> &Self::Target {
        &self.shared_context
    }
}

impl DerefMut for ClientContext {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.shared_context
    }
}

#[cfg(feature = "serde1")]
mod absolute_to_relative_time {
    pub use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub use std::time::{Duration, Instant};

    pub fn serialize<S>(deadline: &Instant, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let deadline = deadline.duration_since(Instant::now());
        deadline.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Instant, D::Error>
    where
        D: Deserializer<'de>,
    {
        let deadline = Duration::deserialize(deserializer)?;
        Ok(Instant::now() + deadline)
    }

    #[cfg(test)]
    #[derive(serde::Serialize, serde::Deserialize)]
    struct AbsoluteToRelative(#[serde(with = "self")] Instant);

    #[test]
    fn test_serialize() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(10);
        let serialized_deadline = bincode::serde::encode_to_vec(
            AbsoluteToRelative(deadline),
            bincode::config::standard(),
        )
        .unwrap();
        let (deserialized_deadline, _): (Duration, _) =
            bincode::decode_from_slice(&serialized_deadline, bincode::config::standard()).unwrap();
        // TODO: how to avoid flakiness?
        assert!(deserialized_deadline > Duration::from_secs(9));
    }

    #[test]
    fn test_deserialize() {
        let deadline = Duration::from_secs(10);
        let serialized_deadline =
            bincode::encode_to_vec(deadline, bincode::config::standard()).unwrap();
        let (AbsoluteToRelative(deserialized_deadline), _) =
            bincode::serde::decode_from_slice(&serialized_deadline, bincode::config::standard())
                .unwrap();
        // TODO: how to avoid flakiness?
        assert!(deserialized_deadline > Instant::now() + Duration::from_secs(9));
    }
}

assert_impl_all!(SharedContext: Send, Sync);

fn ten_seconds_from_now() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

#[derive(Clone)]
struct Deadline(Instant);

impl Default for Deadline {
    fn default() -> Self {
        Self(ten_seconds_from_now())
    }
}

impl SharedContext {
    /// Returns the context for the current request, or a default Context if no request is active.
    pub fn current() -> Self {
        let span = tracing::Span::current();
        Self {
            trace_context: trace::Context::try_from(&span)
                .unwrap_or_else(|_| trace::Context::default()),
            deadline: span
                .context()
                .get::<Deadline>()
                .cloned()
                .unwrap_or_default()
                .0,
        }
    }

    /// Returns the ID of the request-scoped trace.
    pub fn trace_id(&self) -> &TraceId {
        &self.trace_context.trace_id
    }
}

/// An extension trait for [`tracing::Span`] for propagating tarpc Contexts.
pub(crate) trait SpanExt {
    /// Sets the given context on this span. Newly-created spans will be children of the given
    /// context's trace context.
    fn set_context(&self, context: &SharedContext);
}

impl SpanExt for tracing::Span {
    fn set_context(&self, context: &SharedContext) {
        self.set_parent(
            opentelemetry::Context::new()
                .with_remote_span_context(opentelemetry::trace::SpanContext::new(
                    opentelemetry::trace::TraceId::from(context.trace_context.trace_id),
                    opentelemetry::trace::SpanId::from(context.trace_context.span_id),
                    opentelemetry::trace::TraceFlags::from(context.trace_context.sampling_decision),
                    true,
                    opentelemetry::trace::TraceState::default(),
                ))
                .with_value(Deadline(context.deadline)),
        );
    }
}
