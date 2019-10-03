// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Provides a request context that carries a deadline and trace context. This context is sent from
//! client to server and is used by the server to enforce response deadlines.

use std::time::{Duration, SystemTime};
use trace::{self, TraceId};

/// A request context that carries request-scoped information like deadlines and trace information.
/// It is sent from client to server and is used by the server to enforce response deadlines.
///
/// The context should not be stored directly in a server implementation, because the context will
/// be different for each request in scope.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
pub struct Context {
    /// When the client expects the request to be complete by. The server should cancel the request
    /// if it is not complete by this time.
    #[cfg_attr(
        feature = "serde1",
        serde(serialize_with = "crate::util::serde::serialize_epoch_secs")
    )]
    #[cfg_attr(
        feature = "serde1",
        serde(deserialize_with = "crate::util::serde::deserialize_epoch_secs")
    )]
    #[cfg_attr(feature = "serde1", serde(default = "ten_seconds_from_now"))]
    pub deadline: SystemTime,
    /// Uniquely identifies requests originating from the same source.
    /// When a service handles a request by making requests itself, those requests should
    /// include the same `trace_id` as that included on the original request. This way,
    /// users can trace related actions across a distributed system.
    pub trace_context: trace::Context,
    #[doc(hidden)]
    #[cfg_attr(feature = "serde1", serde(skip_serializing, default))]
    pub(crate) _non_exhaustive: (),
}

#[cfg(feature = "serde1")]
fn ten_seconds_from_now() -> SystemTime {
    return SystemTime::now() + Duration::from_secs(10);
}

/// Returns the context for the current request, or a default Context if no request is active.
// TODO: populate Context with request-scoped data, with default fallbacks.
pub fn current() -> Context {
    Context {
        deadline: SystemTime::now() + Duration::from_secs(10),
        trace_context: trace::Context::new_root(),
        _non_exhaustive: (),
    }
}

impl Context {
    /// Returns the ID of the request-scoped trace.
    pub fn trace_id(&self) -> &TraceId {
        &self.trace_context.trace_id
    }
}
