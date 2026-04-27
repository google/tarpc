// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Fory-friendly envelope types for the `serde_transport::fory` codec path.
//!
//! These mirror tarpc's native `ClientMessage<T>`, `Response<T>`, `Request<T>`,
//! and `ServerError` types. The native types are unsuitable for fory because
//! they reference `std::result::Result`, `std::time::Instant`, and
//! `std::io::ErrorKind` — none of which fory-core 0.17 provides
//! `fory::Serializer` impls for.
//!
//! Conversions between native and wrapper types are field-by-field and lossless
//! for the round-trip path the transport actually uses. Schema drift is
//! caught by the codec parity test (Task 3.5).
//!
//! ## `io::ErrorKind` wire encoding
//!
//! The u32 discriminants match tarpc's own serde encoding in `util/serde.rs`:
//!
//! | u32 | io::ErrorKind        |
//! |-----|----------------------|
//! |   0 | NotFound             |
//! |   1 | PermissionDenied     |
//! |   2 | ConnectionRefused    |
//! |   3 | ConnectionReset      |
//! |   4 | ConnectionAborted    |
//! |   5 | NotConnected         |
//! |   6 | AddrInUse            |
//! |   7 | AddrNotAvailable     |
//! |   8 | BrokenPipe           |
//! |   9 | AlreadyExists        |
//! |  10 | WouldBlock           |
//! |  11 | InvalidInput         |
//! |  12 | InvalidData          |
//! |  13 | TimedOut             |
//! |  14 | WriteZero            |
//! |  15 | Interrupted          |
//! |  16 | Other                |
//! |  17 | UnexpectedEof        |
//! |  _  | Other (fallback)     |
//!
//! ## Deadline encoding
//!
//! `context::Context.deadline` is a `std::time::Instant` (monotonic, local-only).
//! For the wire we encode it as the number of nanoseconds remaining from `Instant::now()`
//! at serialize time (i.e. `deadline.saturating_duration_since(now).as_nanos() as u64`).
//! A value of 0 means "already expired or no deadline". The receiver reconstructs
//! `Instant::now() + Duration::from_nanos(remaining_ns)`.

use std::io;
use std::time::{Duration, Instant};

use fory::{ForyDefault, ForyObject, Serializer};

use crate::{ClientMessage, Request, Response, ServerError, context, trace};

// ---------------------------------------------------------------------------
// Wrapper types
// ---------------------------------------------------------------------------

/// Fory-serializable mirror of `trace::Context`.
#[derive(Debug, Clone, ForyObject)]
pub struct ForyTraceContext {
    /// Raw u128 bits of the `TraceId`.
    pub trace_id: u128,
    /// Raw u64 bits of the `SpanId`.
    pub span_id: u64,
    /// Sampling decision: 0 = Unsampled, 1 = Sampled.
    pub sampling: u8,
}

/// Fory-serializable mirror of `ServerError`.
///
/// `kind` is encoded as a u32 per the table in the module-level docs.
#[derive(Debug, Clone, ForyObject)]
pub struct ForyServerError {
    /// io::ErrorKind discriminant value (see module docs for mapping).
    pub kind: u32,
    /// Human-readable error detail.
    pub detail: String,
}

/// Fory-serializable analog of `Result<T, ServerError>`.
///
/// fory-core 0.17 has no `Serializer` impl for `std::result::Result`, so we
/// define a dedicated two-variant enum.
#[derive(Debug, Clone, ForyObject)]
pub enum ForyResult<T: Serializer + ForyDefault + 'static> {
    /// Successful response payload.
    Ok(T),
    /// Server-side error.
    Err(ForyServerError),
}

/// Fory-serializable mirror of `Request<T>`.
///
/// `context::Context.deadline` (an `Instant`) is replaced by `deadline_ns`:
/// nanoseconds remaining from the point of serialization. See module docs.
#[derive(Debug, Clone, ForyObject)]
pub struct ForyRequest<T: Serializer + ForyDefault + 'static> {
    /// Request ID — unique within a single channel.
    pub id: u64,
    /// Trace context (mirrors `context.trace_context`).
    pub trace: ForyTraceContext,
    /// Deadline as nanoseconds remaining (from serialize time).  0 = expired / no deadline.
    pub deadline_ns: u64,
    /// Request body.
    pub message: T,
}

/// Fory-serializable mirror of `Response<T>`.
#[derive(Debug, Clone, ForyObject)]
pub struct ForyResponse<T: Serializer + ForyDefault + 'static> {
    /// ID of the request this is responding to.
    pub request_id: u64,
    /// Response body or server error.
    pub message: ForyResult<T>,
}

/// Fory-serializable mirror of `ClientMessage<T>`.
#[derive(Debug, Clone, ForyObject)]
pub enum ForyClientMessage<T: Serializer + ForyDefault + 'static> {
    /// A new request from the client.
    Request(ForyRequest<T>),
    /// Cancellation of an in-flight request.
    Cancel {
        /// Trace context associated with the original request.
        trace: ForyTraceContext,
        /// ID of the request to cancel.
        request_id: u64,
    },
}

// ---------------------------------------------------------------------------
// Conversions: native → wrapper
// ---------------------------------------------------------------------------

impl From<trace::SamplingDecision> for u8 {
    fn from(d: trace::SamplingDecision) -> u8 {
        match d {
            trace::SamplingDecision::Sampled => 1,
            trace::SamplingDecision::Unsampled => 0,
        }
    }
}

impl From<u8> for trace::SamplingDecision {
    fn from(v: u8) -> trace::SamplingDecision {
        if v == 1 {
            trace::SamplingDecision::Sampled
        } else {
            trace::SamplingDecision::Unsampled
        }
    }
}

impl From<&trace::Context> for ForyTraceContext {
    fn from(tc: &trace::Context) -> Self {
        ForyTraceContext {
            trace_id: u128::from(tc.trace_id),
            span_id: u64::from(tc.span_id),
            sampling: u8::from(tc.sampling_decision),
        }
    }
}

impl From<ForyTraceContext> for trace::Context {
    fn from(ftc: ForyTraceContext) -> Self {
        trace::Context {
            trace_id: trace::TraceId::from(ftc.trace_id),
            span_id: trace::SpanId::from(ftc.span_id),
            sampling_decision: trace::SamplingDecision::from(ftc.sampling),
        }
    }
}

/// Encode `io::ErrorKind` as u32 using the same table as `util::serde`.
pub fn error_kind_to_u32(kind: io::ErrorKind) -> u32 {
    use io::ErrorKind::*;
    match kind {
        NotFound => 0,
        PermissionDenied => 1,
        ConnectionRefused => 2,
        ConnectionReset => 3,
        ConnectionAborted => 4,
        NotConnected => 5,
        AddrInUse => 6,
        AddrNotAvailable => 7,
        BrokenPipe => 8,
        AlreadyExists => 9,
        WouldBlock => 10,
        InvalidInput => 11,
        InvalidData => 12,
        TimedOut => 13,
        WriteZero => 14,
        Interrupted => 15,
        Other => 16,
        UnexpectedEof => 17,
        _ => 16, // map unknown variants to Other
    }
}

/// Decode u32 back to `io::ErrorKind`.
pub fn u32_to_error_kind(v: u32) -> io::ErrorKind {
    use io::ErrorKind::*;
    match v {
        0 => NotFound,
        1 => PermissionDenied,
        2 => ConnectionRefused,
        3 => ConnectionReset,
        4 => ConnectionAborted,
        5 => NotConnected,
        6 => AddrInUse,
        7 => AddrNotAvailable,
        8 => BrokenPipe,
        9 => AlreadyExists,
        10 => WouldBlock,
        11 => InvalidInput,
        12 => InvalidData,
        13 => TimedOut,
        14 => WriteZero,
        15 => Interrupted,
        16 => Other,
        17 => UnexpectedEof,
        _ => Other,
    }
}

impl From<&ServerError> for ForyServerError {
    fn from(se: &ServerError) -> Self {
        ForyServerError {
            kind: error_kind_to_u32(se.kind),
            detail: se.detail.clone(),
        }
    }
}

impl From<ForyServerError> for ServerError {
    fn from(fse: ForyServerError) -> Self {
        ServerError {
            kind: u32_to_error_kind(fse.kind),
            detail: fse.detail,
        }
    }
}

impl<T: Serializer + ForyDefault + Clone + 'static> From<&Request<T>> for ForyRequest<T> {
    fn from(req: &Request<T>) -> Self {
        let now = Instant::now();
        let deadline_ns = req
            .context
            .deadline
            .checked_duration_since(now)
            .map(|d| d.as_nanos().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        ForyRequest {
            id: req.id,
            trace: ForyTraceContext::from(&req.context.trace_context),
            deadline_ns,
            message: req.message.clone(),
        }
    }
}

impl<T: Serializer + ForyDefault + 'static> From<ForyRequest<T>> for Request<T> {
    fn from(fr: ForyRequest<T>) -> Self {
        let deadline = Instant::now() + Duration::from_nanos(fr.deadline_ns);
        Request {
            context: context::Context {
                deadline,
                trace_context: trace::Context::from(fr.trace),
            },
            id: fr.id,
            message: fr.message,
        }
    }
}

impl<T: Serializer + ForyDefault + 'static> From<Result<T, ServerError>> for ForyResult<T> {
    fn from(r: Result<T, ServerError>) -> Self {
        match r {
            Ok(v) => ForyResult::Ok(v),
            Err(e) => ForyResult::Err(ForyServerError::from(&e)),
        }
    }
}

impl<T: Serializer + ForyDefault + 'static> From<ForyResult<T>> for Result<T, ServerError> {
    fn from(fr: ForyResult<T>) -> Self {
        match fr {
            ForyResult::Ok(v) => Ok(v),
            ForyResult::Err(e) => Err(ServerError::from(e)),
        }
    }
}

impl<T: Serializer + ForyDefault + 'static> From<&Response<T>> for ForyResponse<T>
where
    T: Clone,
{
    fn from(resp: &Response<T>) -> Self {
        ForyResponse {
            request_id: resp.request_id,
            message: ForyResult::from(resp.message.clone()),
        }
    }
}

impl<T: Serializer + ForyDefault + 'static> From<ForyResponse<T>> for Response<T> {
    fn from(fr: ForyResponse<T>) -> Self {
        Response {
            request_id: fr.request_id,
            message: Result::from(fr.message),
        }
    }
}

impl<T: Serializer + ForyDefault + Clone + 'static> From<&ClientMessage<T>>
    for ForyClientMessage<T>
{
    fn from(msg: &ClientMessage<T>) -> Self {
        match msg {
            ClientMessage::Request(req) => {
                ForyClientMessage::Request(ForyRequest::from(req))
            }
            ClientMessage::Cancel {
                trace_context,
                request_id,
            } => ForyClientMessage::Cancel {
                trace: ForyTraceContext::from(trace_context),
                request_id: *request_id,
            },
        }
    }
}

impl<T: Serializer + ForyDefault + 'static> From<ForyClientMessage<T>> for ClientMessage<T> {
    fn from(fm: ForyClientMessage<T>) -> Self {
        match fm {
            ForyClientMessage::Request(fr) => ClientMessage::Request(Request::from(fr)),
            ForyClientMessage::Cancel { trace, request_id } => ClientMessage::Cancel {
                trace_context: trace::Context::from(trace),
                request_id,
            },
        }
    }
}
