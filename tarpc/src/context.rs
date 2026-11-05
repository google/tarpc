// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a request context that carries a deadline and trace context. This context is sent from
//! client to server and is used by the server to enforce response deadlines.

use crate::trace::{self, TraceId};
use static_assertions::assert_impl_all;
use std::{
    convert::TryFrom,
    time::{Duration, Instant},
};

/// A request context that carries request-scoped information like deadlines and trace information.
/// It is sent from client to server and is used by the server to enforce response deadlines.
///
/// The context should not be stored directly in a server implementation, because the context will
/// be different for each request in scope.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
#[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
pub struct Context {
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
    pub trace_context: trace::Context,
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

assert_impl_all!(Context: Send, Sync);

fn ten_seconds_from_now() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

/// Returns the context for the current request, or a default Context if no request is active.
pub fn current() -> Context {
    Context::current()
}

impl Context {
    /// Returns the context for the current request, or a default Context if no request is active.
    pub fn current() -> Self {
        let span = tracing::Span::current();
        Self {
            trace_context: trace::Context::try_from(&span)
                .unwrap_or_else(|_| trace::Context::default()),
            deadline: span.deadline().unwrap_or_else(ten_seconds_from_now),
        }
    }

    /// Returns the ID of the request-scoped trace.
    pub fn trace_id(&self) -> &TraceId {
        &self.trace_context.trace_id
    }
}

/// An extension trait for [`tracing::Span`] for propagating tarpc Contexts.
///
/// This shims `tracing-opentelemetry` when the `opentelemetry` feature is enabled and returns
/// no span state otherwise. The `tracing` dependency itself is always enabled.
pub(crate) trait SpanExt {
    /// Reads the OpenTelemetry trace context, keeping `TryFrom<&tracing::Span>` available
    /// without the `opentelemetry` feature. Returns `NoActiveSpan` when that feature is disabled
    /// or the span has no active OpenTelemetry context.
    fn trace_context(&self) -> Result<trace::Context, trace::NoActiveSpan>;

    /// Reads the request deadline stored in the OpenTelemetry context. Returns `None` when
    /// the `opentelemetry` feature is disabled or no deadline is stored, so callers can use
    /// the default deadline.
    fn deadline(&self) -> Option<Instant>;

    /// Sets the given context on this span. Newly-created spans will be children of the given
    /// context's trace context.
    ///
    /// Returns an error if the `opentelemetry` feature is disabled, the OpenTelemetry layer
    /// is missing, the span is disabled, or the span has already started.
    fn set_context(&self, context: &Context) -> Result<(), SetContextError>;
}

/// Explains why a request context could not be attached to a span.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SetContextError {
    #[cfg(not(feature = "opentelemetry"))]
    #[error("OpenTelemetry feature is disabled")]
    Disabled,
    #[cfg(feature = "opentelemetry")]
    #[error(transparent)]
    OpenTelemetry(#[from] tracing_opentelemetry::SetParentError),
}

#[cfg(not(feature = "opentelemetry"))]
impl SpanExt for tracing::Span {
    fn trace_context(&self) -> Result<trace::Context, trace::NoActiveSpan> {
        Err(trace::NoActiveSpan)
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn set_context(&self, _: &Context) -> Result<(), SetContextError> {
        // Plain tracing spans cannot store a distributed context; the request still carries it.
        Err(SetContextError::Disabled)
    }
}

#[cfg(test)]
mod tests {
    use super::current;
    use crate::trace;
    use std::time::{Duration, Instant};

    #[test]
    fn current_without_subscriber() {
        tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
            let earliest_deadline = Instant::now() + Duration::from_secs(10);
            let context = current();
            let latest_deadline = Instant::now() + Duration::from_secs(10);

            assert_eq!(context.trace_context, trace::Context::default());
            assert!((earliest_deadline..=latest_deadline).contains(&context.deadline));
        });
    }
}
