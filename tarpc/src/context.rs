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
use tracing_opentelemetry::OpenTelemetrySpanExt;

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
    pub deadline: SystemTime,
    /// Uniquely identifies requests originating from the same source.
    /// When a service handles a request by making requests itself, those requests should
    /// include the same `trace_id` as that included on the original request. This way,
    /// users can trace related actions across a distributed system.
    pub trace_context: trace::Context,
}

assert_impl_all!(Context: Send, Sync);

fn ten_seconds_from_now() -> SystemTime {
    SystemTime::now() + Duration::from_secs(10)
}

/// Returns the context for the current request, or a default Context if no request is active.
pub fn current() -> Context {
    Context::current()
}

#[derive(Clone)]
struct Deadline(SystemTime);

impl Default for Deadline {
    fn default() -> Self {
        Self(ten_seconds_from_now())
    }
}

impl Context {
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
                .with_value(Deadline(context.deadline)),
        );
    }
}
