// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#![deny(missing_docs, missing_debug_implementations)]

//! Provides building blocks for tracing distributed programs.
//!
//! A trace is logically a tree of causally-related events called spans. Traces are tracked via a
//! [context](Context) that identifies the current trace, span, and parent of the current span.  In
//! distributed systems, a context can be sent from client to server to connect events occurring on
//! either side.
//!
//! This crate's design is based on [opencensus
//! tracing](https://opencensus.io/core-concepts/tracing/).

use rand::Rng;
use std::{
    fmt::{self, Formatter},
    mem,
};

/// A context for tracing the execution of processes, distributed or otherwise.
///
/// Consists of a span identifying an event, an optional parent span identifying a causal event
/// that triggered the current span, and a trace with which all related spans are associated.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize)
)]
pub struct Context {
    /// An identifier of the trace associated with the current context. A trace ID is typically
    /// created at a root span and passed along through all causal events.
    pub trace_id: TraceId,
    /// An identifier of the current span. In typical RPC usage, a span is created by a client
    /// before making an RPC, and the span ID is sent to the server. The server is free to create
    /// its own spans, for which it sets the client's span as the parent span.
    pub span_id: SpanId,
    /// An identifier of the span that originated the current span. For example, if a server sends
    /// an RPC in response to a client request that included a span, the server would create a span
    /// for the RPC and set its parent to the span_id in the incoming request's context.
    ///
    /// If `parent_id` is `None`, then this is a root context.
    pub parent_id: Option<SpanId>,
}

/// A 128-bit UUID identifying a trace. All spans caused by the same originating span share the
/// same trace ID.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize)
)]
pub struct TraceId(u128);

/// A 64-bit identifier of a span within a trace. The identifier is unique within the span's trace.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize)
)]
pub struct SpanId(u64);

impl Context {
    /// Constructs a new root context. A root context is one with no parent span.
    pub fn new_root() -> Self {
        let rng = &mut rand::thread_rng();
        Context {
            trace_id: TraceId::random(rng),
            span_id: SpanId::random(rng),
            parent_id: None,
        }
    }
}

impl TraceId {
    /// Returns a random trace ID that can be assumed to be globally unique if `rng` generates
    /// actually-random numbers.
    pub fn random<R: Rng>(rng: &mut R) -> Self {
        TraceId((rng.next_u64() as u128) << mem::size_of::<u64>() | rng.next_u64() as u128)
    }
}

impl SpanId {
    /// Returns a random span ID that can be assumed to be unique within a single trace.
    pub fn random<R: Rng>(rng: &mut R) -> Self {
        SpanId(rng.next_u64())
    }
}

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:02x}", self.0)?;
        Ok(())
    }
}

impl fmt::Display for SpanId {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:02x}", self.0)?;
        Ok(())
    }
}
