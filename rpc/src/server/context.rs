use crate::client;
use std::{net::SocketAddr, time::SystemTime};
use trace::{self, TraceId};

/// Auxiliary context for a request.
pub struct Context {
    /// When the client expects the request to be complete by. The server should cancel the request
    /// if it is not complete by this time.
    pub deadline: SystemTime,
    /// The client that sent the request.
    pub client_addr: SocketAddr,
    /// The trace context.
    pub trace_context: trace::Context,
}

impl Context {
    pub fn trace_id(&self) -> &TraceId {
        &self.trace_context.trace_id
    }
}

impl<'a> Into<client::Context> for &'a Context {
    fn into(self) -> client::Context {
        client::Context {
            deadline: self.deadline,
            trace_context: self.trace_context,
        }
    }
}
