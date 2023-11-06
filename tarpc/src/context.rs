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
use std::{
    convert::TryFrom,
    time::{Duration, SystemTime},
};
use std::hash::{Hash, Hasher};
use std::ops::{Deref, DerefMut};
use anymap::any::CloneAny;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// A request context that carries request-scoped information like deadlines and trace information.
/// It is sent from client to server and is used by the server to enforce response deadlines.
///
/// The context should not be stored directly in a server implementation, because the context will
/// be different for each request in scope.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
#[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
pub struct Context {
    /// When the client expects the request to be complete by. The server should cancel the request
    /// if it is not complete by this time.
    // Serialized as a Duration to prevent clock skew issues.
    #[cfg_attr(feature = "serde1", serde(with = "absolute_to_relative_time"))]
    pub deadline: Deadline,
    /// Uniquely identifies requests originating from the same source.
    /// When a service handles a request by making requests itself, those requests should
    /// include the same `trace_id` as that included on the original request. This way,
    /// users can trace related actions across a distributed system.
    pub trace_context: trace::Context,

    /// Any extra information can be requested
    #[cfg_attr(feature = "serde1", serde(skip))]
    pub extensions: Extensions
}

impl PartialEq for Context {
    fn eq(&self, other: &Self) -> bool {
        self.trace_context == other.trace_context && self.deadline == other.deadline
    }
}

impl Eq for Context { }

impl Hash for Context {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.trace_context.hash(state);
        self.deadline.hash(state);
    }
}

#[cfg(feature = "serde1")]
mod absolute_to_relative_time {
    pub use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub use std::time::{Duration, SystemTime};
    use crate::context::Deadline;

    pub fn serialize<S>(deadline: &Deadline, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let deadline = deadline.0
            .duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO);
        deadline.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Deadline, D::Error>
    where
        D: Deserializer<'de>,
    {
        let deadline = Duration::deserialize(deserializer)?;
        Ok(Deadline(SystemTime::now() + deadline))
    }

    #[cfg(test)]
    #[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
    struct AbsoluteToRelative(#[serde(with = "self")] Deadline);

    #[test]
    fn test_serialize() {
        let now = SystemTime::now();
        let deadline = Deadline(now + Duration::from_secs(10));
        let serialized_deadline = bincode::serialize(&AbsoluteToRelative(deadline)).unwrap();
        let deserialized_deadline: Duration = bincode::deserialize(&serialized_deadline).unwrap();
        // TODO: how to avoid flakiness?
        assert!(deserialized_deadline > Duration::from_secs(9));
    }

    #[test]
    fn test_deserialize() {
        let deadline = Duration::from_secs(10);
        let serialized_deadline = bincode::serialize(&deadline).unwrap();
        let AbsoluteToRelative(deserialized_deadline) =
            bincode::deserialize(&serialized_deadline).unwrap();
        // TODO: how to avoid flakiness?
        assert!(*deserialized_deadline > SystemTime::now() + Duration::from_secs(9));
    }
}

assert_impl_all!(Context: Send, Sync);

fn ten_seconds_from_now() -> Deadline {
    Deadline(SystemTime::now() + Duration::from_secs(10))
}

/// Returns the context for the current request, or a default Context if no request is active.
pub fn current() -> Context {
    Context::current()
}

/// Deadline for executing a request
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Deadline(pub(self) SystemTime);

impl Default for Deadline {
    fn default() -> Self {
        ten_seconds_from_now()
    }
}

impl Deref for Deadline {
    type Target = SystemTime;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Deadline {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Deadline {
    /// Creates a new deadline
    pub fn new(t: SystemTime) -> Deadline {
        Deadline(t)
    }
}

/// Extensions associated with a request
#[derive(Clone, Debug)]
pub struct Extensions(anymap::Map<dyn CloneAny + Sync + Send>);

impl Default for Extensions {
    fn default() -> Self {
        Self(anymap::Map::new())
    }
}

impl Deref for Extensions {
    type Target = anymap::Map<dyn CloneAny + Sync + Send>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Extensions {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Context {
    /// Returns the context for the current request, or a default Context if no request is active.
    pub fn current() -> Self {
        let span = tracing::Span::current();

        Self {
            trace_context: trace::Context::try_from(&span)
                .unwrap_or_else(|_| trace::Context::default()),
            extensions: Default::default(), // span is always cloned so saving this doesn't make sense.
            deadline: span
                .context()
                .get::<Deadline>()
                .cloned()
                .unwrap_or_default(),
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
    fn set_context(&self, context: &Context);
}

impl SpanExt for tracing::Span {
    fn set_context(&self, context: &Context) {
        self.set_parent(
            opentelemetry::Context::new()
                .with_remote_span_context(opentelemetry::trace::SpanContext::new(
                    opentelemetry::trace::TraceId::from(context.trace_context.trace_id),
                    opentelemetry::trace::SpanId::from(context.trace_context.span_id),
                    opentelemetry::trace::TraceFlags::from(context.trace_context.sampling_decision),
                    true,
                    opentelemetry::trace::TraceState::default(),
                ))
                .with_value(context.deadline.clone())
        );
    }
}
