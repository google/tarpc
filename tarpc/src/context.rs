// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a request context that carries a deadline and trace context. This context is sent from
//! client to server and is used by the server to enforce response deadlines.

use crate::trace::{self, TraceId};
use static_assertions::assert_impl_all;
use std::time::{Duration, SystemTime};

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

tokio::task_local! {
    static CURRENT_CONTEXT: Context;
}

fn ten_seconds_from_now() -> SystemTime {
    SystemTime::now() + Duration::from_secs(10)
}

/// Returns the context for the current request, or a default Context if no request is active.
pub fn current() -> Context {
    Context::current()
}

impl Context {
    /// Returns a Context containing a new root trace context and a default deadline.
    pub fn new_root() -> Self {
        Self {
            deadline: ten_seconds_from_now(),
            trace_context: trace::Context::new_root(),
        }
    }

    /// Returns the context for the current request, or a default Context if no request is active.
    pub fn current() -> Self {
        CURRENT_CONTEXT
            .try_with(Self::clone)
            .unwrap_or_else(|_| Self::new_root())
    }

    /// Returns the ID of the request-scoped trace.
    pub fn trace_id(&self) -> &TraceId {
        &self.trace_context.trace_id
    }

    /// Run a future with this context as the current context.
    pub async fn scope<F>(self, f: F) -> F::Output
    where
        F: std::future::Future,
    {
        CURRENT_CONTEXT.scope(self, f).await
    }
}

#[cfg(test)]
use {
    assert_matches::assert_matches, futures::prelude::*, futures_test::task::noop_context,
    std::task::Poll,
};

#[test]
fn context_current_has_no_parent() {
    let ctx = current();
    assert_matches!(ctx.trace_context.parent_id, None);
}

#[test]
fn context_root_has_no_parent() {
    let ctx = Context::new_root();
    assert_matches!(ctx.trace_context.parent_id, None);
}

#[test]
fn context_scope() {
    let ctx = Context::new_root();
    let mut ctx_copy = Box::pin(ctx.scope(async { current() }));
    assert_matches!(ctx_copy.poll_unpin(&mut noop_context()),
                    Poll::Ready(Context {
                        deadline,
                        trace_context: trace::Context { trace_id, span_id, parent_id },
                    }) if deadline == ctx.deadline
                    && trace_id == ctx.trace_context.trace_id
                    && span_id == ctx.trace_context.span_id
                    && parent_id == ctx.trace_context.parent_id);
}
