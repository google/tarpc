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
pub struct DefaultContext {
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

/// A shared, on-the-wire request context.
///
/// `SharedContext` defines the minimal interface required for contexts that are
/// transmitted between peers as part of an RPC call.
///
/// Implementations of this trait represent the *wire-level* context: a
/// portable, serializable view of request-scoped metadata such as deadlines
/// and tracing information. Service implementations are free to define
/// richer context types as part of their contract, for example to implement:
///
/// - ephemeral sessions
/// - authentication / authorization data
/// - cookie-like state
/// - other application-specific metadata
///
/// while preserving a common core required by the RPC runtime.
pub trait SharedContext {
    /// Returns the absolute deadline for the request.
    ///
    /// The deadline represents the latest instant at which the request
    /// should still be processed. RPC runtimes and middleware may use this
    /// value to enforce timeouts, cancel in-flight work, or reject requests
    /// that have already expired.
    fn deadline(&self) -> Instant;

    /// Returns the distributed tracing context associated with the request.
    ///
    /// This context is propagated across RPC boundaries to enable
    /// end-to-end request tracing and correlation.
    //TODO: May want to remove this in the long run from the default context, may need https://github.com/rust-lang/rust/issues/144361 for that.
    fn trace_context(&self) -> trace::Context;

    /// Updates the distributed tracing context.
    ///
    /// This is typically used by middleware to attach spans, propagate
    /// trace identifiers, or replace the tracing context after deserialization.
    //TODO: May want to remove this in the long run from the default context, may need https://github.com/rust-lang/rust/issues/144361 for that.
    fn set_trace_context(&mut self, trace_context: trace::Context);
}

impl SharedContext for DefaultContext {
    fn deadline(&self) -> Instant {
        self.deadline
    }

    fn trace_context(&self) -> trace::Context {
        self.trace_context
    }

    fn set_trace_context(&mut self, trace_context: trace::Context) {
        self.trace_context = trace_context;
    }
}

/// Extracts a wire-level shared context contained within a client or server context.
///
/// `ExtractContext` defines a mapping between an internal
/// context representation and a *shared* context type (`Ctx`) that is
/// suitable for serialization and transmission over the wire.
///
/// The shared context typically represents the minimal, stable data
/// exchanged between the client and server, while the
/// implementing type may contain additional, local side only state or
/// a different internal structure.
///
/// # Design notes
///
/// If a type implements both UpdateContext and ExtractContext, it is expected that
/// `foo.update(v).extract() == v` will hold.
// TODO: Revisit this trait once try_as_dyn is stabilized, https://github.com/rust-lang/rust/issues/29661.
pub trait ExtractContext<Ctx> {
    /// Extracts the inner context from the internal state.
    ///
    /// This method is typically called before sending the context over
    /// the wire, or just after receiving it. The returned value should contain
    /// all information required by the remote side to reconstruct or update its own
    /// local context.
    fn extract(&self) -> Ctx;

}

/// Updates a wire-level shared context contained within a client context.
///
/// `ExtractContext` defines a mapping between a *shared* context type (`Ctx`)
/// and an internal context representation
///
/// The shared context typically represents the minimal, stable data
/// exchanged between the client and server, while the
/// implementing type may contain additional, local side only state or
/// a different internal structure.
///
/// # Design notes
///
/// It is expected that `ctx.update(shared_ctx).extract() == shared_ctx` will always hold.
///
// TODO: Revisit this trait once try_as_dyn is stabilized, https://github.com/rust-lang/rust/issues/29661.
pub trait UpdateContext<Ctx>: ExtractContext<Ctx> {
    /// Updates the internal state from an inner context value.
    ///
    /// This method is typically called after executing a request and before
    /// sending the updated context over the wire. Implementations should apply
    /// the provided value to their internal representation, updating any derived or
    /// local-only state as necessary.
    fn update(&mut self, value: Ctx);
}

impl<T> ExtractContext<T> for T
where
    T: Clone,
{
    fn extract(&self) -> T {
        self.clone()
    }
}

impl<T: ExtractContext<T>> UpdateContext<T> for T {
    fn update(&mut self, value: T) {
        *self = value
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

assert_impl_all!(DefaultContext: Send, Sync);

fn ten_seconds_from_now() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

/// Returns the context for the current request, or a default Context if no request is active.
pub fn current() -> DefaultContext {
    DefaultContext::current()
}

#[derive(Clone)]
struct Deadline(Instant);

impl Default for Deadline {
    fn default() -> Self {
        Self(ten_seconds_from_now())
    }
}

impl DefaultContext {
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
    fn set_context<T: SharedContext>(&self, context: &T);
}

impl SpanExt for tracing::Span {
    fn set_context<T: SharedContext>(&self, context: &T) {
        self.set_parent(
            opentelemetry::Context::new()
                .with_remote_span_context(opentelemetry::trace::SpanContext::new(
                    opentelemetry::trace::TraceId::from(context.trace_context().trace_id),
                    opentelemetry::trace::SpanId::from(context.trace_context().span_id),
                    opentelemetry::trace::TraceFlags::from(
                        context.trace_context().sampling_decision,
                    ),
                    true,
                    opentelemetry::trace::TraceState::default(),
                ))
                .with_value(Deadline(context.deadline())),
        );
    }
}
