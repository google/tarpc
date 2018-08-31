// Copyright 2018 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use std::time::{Duration, SystemTime};
use trace::{self, TraceId};

/// A request context that carries request-scoped information
/// like deadlines and trace information.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Context {
    /// When the client expects the request to be complete by. The server should cancel the request
    /// if it is not complete by this time.
    pub deadline: SystemTime,
    /// Uniquely identifies requests originating from the same source.
    /// When a service handles a request by making requests itself, those requests should
    /// include the same `trace_id` as that included on the original request. This way,
    /// users can trace related actions across a distributed system.
    pub trace_context: trace::Context,
}

/// Returns the context for the current request, or a default Context if no request is active.
// TODO: populate Context with request-scoped data, with default fallbacks.
pub fn current() -> Context {
    Context {
        deadline: SystemTime::now() + Duration::from_secs(10),
        trace_context: trace::Context::new_root(),
    }
}

impl Context {
    /// Returns the ID of the request-scoped trace.
    pub fn trace_id(&self) -> &TraceId {
        &self.trace_context.trace_id
    }
}
