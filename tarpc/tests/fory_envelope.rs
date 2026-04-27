// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Round-trip tests for fory-serializable tarpc envelope types.
//!
//! These tests verify that `ForyClientMessage`, `ForyRequest`, `ForyResponse`,
//! and their helpers can be serialized and deserialized via the fory codec
//! without data loss. They also exercise the `From`/`Into` conversions between
//! the fory wrapper types and tarpc's native envelope types.
//!
//! ## Why primitive generics?
//!
//! The `ForyObject` derive macro assigns a compile-time type index starting from 0
//! for each crate. Registering a user-defined `Body` type (index 0 in the test crate)
//! alongside `ForyTraceContext` (also index 0 in the tarpc lib crate) in the same
//! `TypeResolver` would collide. We avoid this by using builtin primitive types like
//! `u32` or `String` as the generic parameter `T`, because builtin types are registered
//! through `register_internal_serializer` and do not occupy the compile-time index table.
//! Native ↔ wrapper conversion tests use `u32` for the same reason.

#![cfg(feature = "serde-transport-fory")]

use fory::Fory;
use tarpc::serde_transport::fory_envelope::{
    ForyClientMessage, ForyRequest, ForyResponse, ForyResult, ForyServerError, ForyTraceContext,
};

// ---------------------------------------------------------------------------
// Helper: build a Fory instance with the envelope types registered.
//
// We use u32 as the generic parameter for all envelope types to avoid
// compile-time type-index collisions between the test crate and the tarpc lib.
// ---------------------------------------------------------------------------

fn make_fory_u32() -> Fory {
    let mut fory = Fory::default();
    fory.register::<ForyTraceContext>(2).unwrap();
    fory.register::<ForyServerError>(3).unwrap();
    fory.register::<ForyResult<u32>>(4).unwrap();
    fory.register::<ForyRequest<u32>>(5).unwrap();
    fory.register::<ForyResponse<u32>>(6).unwrap();
    fory.register::<ForyClientMessage<u32>>(7).unwrap();
    fory
}

fn make_fory_string() -> Fory {
    let mut fory = Fory::default();
    fory.register::<ForyTraceContext>(2).unwrap();
    fory.register::<ForyServerError>(3).unwrap();
    fory.register::<ForyResult<String>>(4).unwrap();
    fory.register::<ForyRequest<String>>(5).unwrap();
    fory.register::<ForyResponse<String>>(6).unwrap();
    fory.register::<ForyClientMessage<String>>(7).unwrap();
    fory
}

// ---------------------------------------------------------------------------
// Round-trip tests
// ---------------------------------------------------------------------------

#[test]
fn fory_trace_context_round_trip() {
    let mut fory = Fory::default();
    fory.register::<ForyTraceContext>(2).unwrap();

    let original = ForyTraceContext {
        trace_id: 0xDEAD_BEEF_CAFE_BABE_0102_0304_0506_0708_u128,
        span_id: 0xA1B2_C3D4_E5F6_0001_u64,
        sampling: 1,
    };
    let bytes = fory.serialize(&original).unwrap();
    let decoded: ForyTraceContext = fory.deserialize(&bytes).unwrap();
    assert_eq!(decoded.trace_id, original.trace_id);
    assert_eq!(decoded.span_id, original.span_id);
    assert_eq!(decoded.sampling, original.sampling);
}

#[test]
fn fory_server_error_round_trip() {
    let mut fory = Fory::default();
    fory.register::<ForyServerError>(3).unwrap();

    let original = ForyServerError {
        kind: 13, // TimedOut
        detail: "request timed out after 5s".into(),
    };
    let bytes = fory.serialize(&original).unwrap();
    let decoded: ForyServerError = fory.deserialize(&bytes).unwrap();
    assert_eq!(decoded.kind, original.kind);
    assert_eq!(decoded.detail, original.detail);
}

#[test]
fn fory_result_ok_round_trip() {
    let mut fory = Fory::default();
    fory.register::<ForyServerError>(3).unwrap();
    fory.register::<ForyResult<u32>>(4).unwrap();

    let original: ForyResult<u32> = ForyResult::Ok(42u32);
    let bytes = fory.serialize(&original).unwrap();
    let decoded: ForyResult<u32> = fory.deserialize(&bytes).unwrap();
    match decoded {
        ForyResult::Ok(v) => assert_eq!(v, 42u32),
        ForyResult::Err(_) => panic!("expected Ok variant"),
    }
}

#[test]
fn fory_result_err_round_trip() {
    let mut fory = Fory::default();
    fory.register::<ForyServerError>(3).unwrap();
    fory.register::<ForyResult<u32>>(4).unwrap();

    let original: ForyResult<u32> = ForyResult::Err(ForyServerError {
        kind: 0, // NotFound
        detail: "not found".into(),
    });
    let bytes = fory.serialize(&original).unwrap();
    let decoded: ForyResult<u32> = fory.deserialize(&bytes).unwrap();
    match decoded {
        ForyResult::Err(e) => {
            assert_eq!(e.kind, 0);
            assert_eq!(e.detail, "not found");
        }
        ForyResult::Ok(_) => panic!("expected Err variant"),
    }
}

#[test]
fn fory_request_round_trip() {
    let fory = make_fory_u32();
    let original = ForyRequest {
        id: 99,
        trace: ForyTraceContext { trace_id: 100, span_id: 200, sampling: 0 },
        deadline_ns: 5_000_000_000u64,
        message: 777u32,
    };
    let bytes = fory.serialize(&original).unwrap();
    let decoded: ForyRequest<u32> = fory.deserialize(&bytes).unwrap();
    assert_eq!(decoded.id, original.id);
    assert_eq!(decoded.trace.trace_id, original.trace.trace_id);
    assert_eq!(decoded.trace.span_id, original.trace.span_id);
    assert_eq!(decoded.trace.sampling, original.trace.sampling);
    assert_eq!(decoded.deadline_ns, original.deadline_ns);
    assert_eq!(decoded.message, original.message);
}

#[test]
fn fory_response_ok_round_trip() {
    let fory = make_fory_u32();
    let original = ForyResponse { request_id: 1u64, message: ForyResult::Ok(42u32) };
    let bytes = fory.serialize(&original).unwrap();
    let decoded: ForyResponse<u32> = fory.deserialize(&bytes).unwrap();
    assert_eq!(decoded.request_id, 1);
    match decoded.message {
        ForyResult::Ok(v) => assert_eq!(v, 42u32),
        ForyResult::Err(_) => panic!("expected Ok"),
    }
}

#[test]
fn fory_response_err_round_trip() {
    let fory = make_fory_u32();
    let original = ForyResponse::<u32> {
        request_id: 2,
        message: ForyResult::Err(ForyServerError { kind: 1, detail: "permission denied".into() }),
    };
    let bytes = fory.serialize(&original).unwrap();
    let decoded: ForyResponse<u32> = fory.deserialize(&bytes).unwrap();
    assert_eq!(decoded.request_id, 2);
    match decoded.message {
        ForyResult::Err(e) => {
            assert_eq!(e.kind, 1);
            assert_eq!(e.detail, "permission denied");
        }
        ForyResult::Ok(_) => panic!("expected Err"),
    }
}

#[test]
fn fory_client_message_request_round_trip() {
    let fory = make_fory_string();
    let original = ForyClientMessage::Request(ForyRequest {
        id: 42,
        trace: ForyTraceContext { trace_id: 100, span_id: 200, sampling: 1 },
        deadline_ns: 1_000_000_000u64,
        message: "hello".to_string(),
    });

    let bytes = fory.serialize(&original).unwrap();
    let decoded: ForyClientMessage<String> = fory.deserialize(&bytes).unwrap();
    match (&original, &decoded) {
        (ForyClientMessage::Request(o), ForyClientMessage::Request(d)) => {
            assert_eq!(o.id, d.id);
            assert_eq!(o.trace.trace_id, d.trace.trace_id);
            assert_eq!(o.trace.span_id, d.trace.span_id);
            assert_eq!(o.trace.sampling, d.trace.sampling);
            assert_eq!(o.deadline_ns, d.deadline_ns);
            assert_eq!(o.message, d.message);
        }
        _ => panic!("variant mismatch"),
    }
}

#[test]
fn fory_client_message_cancel_round_trip() {
    let fory = make_fory_u32();
    let original: ForyClientMessage<u32> = ForyClientMessage::Cancel {
        trace: ForyTraceContext { trace_id: 55, span_id: 66, sampling: 0 },
        request_id: 123,
    };

    let bytes = fory.serialize(&original).unwrap();
    let decoded: ForyClientMessage<u32> = fory.deserialize(&bytes).unwrap();
    match (&original, &decoded) {
        (
            ForyClientMessage::Cancel { trace: ot, request_id: oid },
            ForyClientMessage::Cancel { trace: dt, request_id: did },
        ) => {
            assert_eq!(ot.trace_id, dt.trace_id);
            assert_eq!(ot.span_id, dt.span_id);
            assert_eq!(oid, did);
        }
        _ => panic!("variant mismatch"),
    }
}

// ---------------------------------------------------------------------------
// Native ↔ wrapper conversion tests
// ---------------------------------------------------------------------------

#[test]
fn native_server_error_round_trip_via_wrapper() {
    use std::io;
    use tarpc::ServerError;
    use tarpc::serde_transport::fory_envelope::{error_kind_to_u32, u32_to_error_kind};

    // All 18 documented error kinds.
    let kinds = [
        io::ErrorKind::NotFound,
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::ConnectionRefused,
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::ConnectionAborted,
        io::ErrorKind::NotConnected,
        io::ErrorKind::AddrInUse,
        io::ErrorKind::AddrNotAvailable,
        io::ErrorKind::BrokenPipe,
        io::ErrorKind::AlreadyExists,
        io::ErrorKind::WouldBlock,
        io::ErrorKind::InvalidInput,
        io::ErrorKind::InvalidData,
        io::ErrorKind::TimedOut,
        io::ErrorKind::WriteZero,
        io::ErrorKind::Interrupted,
        io::ErrorKind::Other,
        io::ErrorKind::UnexpectedEof,
    ];

    for kind in kinds {
        // ServerError::new is the public non-exhaustive constructor.
        let native = ServerError::new(kind, "test".into());
        let wrapper = ForyServerError::from(&native);
        let round_tripped = ServerError::from(wrapper);
        assert_eq!(round_tripped.kind, kind, "kind round-trip failed for {kind:?}");
        assert_eq!(round_tripped.detail, "test");
    }

    // Verify the encoding table matches tarpc's util/serde mapping exactly.
    assert_eq!(error_kind_to_u32(io::ErrorKind::NotFound), 0);
    assert_eq!(error_kind_to_u32(io::ErrorKind::UnexpectedEof), 17);
    assert_eq!(u32_to_error_kind(16), io::ErrorKind::Other);
    assert_eq!(u32_to_error_kind(99), io::ErrorKind::Other); // unknown → Other
}

#[test]
fn native_trace_context_round_trip_via_wrapper() {
    use tarpc::trace::{self, SpanId, TraceId};

    let native = trace::Context {
        trace_id: TraceId::from(0xDEAD_BEEF_u128),
        span_id: SpanId::from(0xCAFE_u64),
        sampling_decision: trace::SamplingDecision::Sampled,
    };

    let wrapper = ForyTraceContext::from(&native);
    assert_eq!(wrapper.trace_id, 0xDEAD_BEEF_u128);
    assert_eq!(wrapper.span_id, 0xCAFE_u64);
    assert_eq!(wrapper.sampling, 1);

    let round_tripped = trace::Context::from(wrapper);
    assert_eq!(u128::from(round_tripped.trace_id), 0xDEAD_BEEF_u128);
    assert_eq!(u64::from(round_tripped.span_id), 0xCAFE_u64);
    assert_eq!(round_tripped.sampling_decision, trace::SamplingDecision::Sampled);
}

#[test]
fn native_request_round_trip_via_wrapper() {
    use std::time::{Duration, Instant};
    use tarpc::Request;
    use tarpc::serde_transport::fory_envelope::ForyRequest;

    // context::Context is #[non_exhaustive] — use ::current() to obtain one.
    // context::current() creates a Context with deadline = now + 10s.
    let ctx = tarpc::context::current();

    let native = Request {
        context: ctx,
        id: 99,
        message: 42u32,
    };

    let wrapper: ForyRequest<u32> = ForyRequest::from(&native);
    assert_eq!(wrapper.id, 99);
    // deadline_ns should be within [9s, 10s] of now.
    assert!(wrapper.deadline_ns > 9_000_000_000, "deadline_ns={}", wrapper.deadline_ns);
    assert!(wrapper.deadline_ns <= 10_000_000_000, "deadline_ns={}", wrapper.deadline_ns);
    assert_eq!(wrapper.message, 42u32);

    let round_tripped: Request<u32> = Request::from(wrapper);
    assert_eq!(round_tripped.id, 99);
    assert_eq!(round_tripped.message, 42u32);
    // The reconstructed deadline should still be within the 10-second window.
    let diff = round_tripped.context.deadline.saturating_duration_since(Instant::now());
    assert!(diff < Duration::from_secs(10), "diff={diff:?}");
    assert!(diff > Duration::from_secs(9), "diff={diff:?}");
}

#[test]
fn native_client_message_cancel_round_trip_via_wrapper() {
    use tarpc::{ClientMessage, trace::{self, SpanId, TraceId}};

    let native: ClientMessage<u32> = ClientMessage::Cancel {
        trace_context: trace::Context {
            trace_id: TraceId::from(77_u128),
            span_id: SpanId::from(88_u64),
            sampling_decision: trace::SamplingDecision::Sampled,
        },
        request_id: 55,
    };

    let wrapper = ForyClientMessage::from(&native);
    let round_tripped: ClientMessage<u32> = ClientMessage::from(wrapper);

    match round_tripped {
        ClientMessage::Cancel { trace_context, request_id } => {
            assert_eq!(u128::from(trace_context.trace_id), 77);
            assert_eq!(u64::from(trace_context.span_id), 88);
            assert_eq!(request_id, 55);
        }
        _ => panic!("expected Cancel variant"),
    }
}
