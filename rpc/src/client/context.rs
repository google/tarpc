use std::time::{Duration, SystemTime};
use trace::{self, TraceId};

#[derive(Debug)]
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

impl Context {
    /// Returns the context for the current request, or a default Context if no request is active.
    // TODO: populate Context with request-scoped data, with default fallbacks.
    pub fn current() -> Self {
        Context {
            deadline: SystemTime::now() + Duration::from_secs(10),
            trace_context: trace::Context::new_root(),
        }
    }

    pub fn trace_id(&self) -> &TraceId {
        &self.trace_context.trace_id
    }
}
