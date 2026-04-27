// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Codec parity / drift guard: round-trip native tarpc envelope types through
//! the fory wrapper types and back, verifying field-level fidelity.
//!
//! Each test exercises the full path:
//!   `native_msg → ForyXxx → fory bytes → ForyXxx → native_msg'`
//! and asserts that `native_msg' ≈ native_msg` (modulo intentionally lossy
//! conversions, e.g. `Instant → relative nanos → re-anchored Instant`).
//!
//! ## Why no custom `Body` struct?
//!
//! The `ForyObject` derive macro assigns a compile-time type index starting
//! from 0 for each crate.  Registering a user-defined `Body` type (index 0 in
//! the test crate) alongside `ForyTraceContext` (also index 0 in the tarpc lib
//! crate) in the same `TypeResolver` would collide.  We avoid this by using
//! builtin primitive types (`u32`, `String`) as the generic parameter `T`,
//! because builtin types are registered through `register_internal_serializer`
//! and do not occupy the compile-time index table.

#![cfg(feature = "serde-transport-fory")]

use fory::Fory;
use std::time::{Duration, Instant};
use tarpc::context;
use tarpc::serde_transport::fory_envelope::{
    ForyClientMessage, ForyRequest, ForyResponse, ForyResult, ForyServerError, ForyTraceContext,
};
use tarpc::trace::{self, SamplingDecision, SpanId, TraceId};
use tarpc::{ClientMessage, Request, Response, ServerError};

// ---------------------------------------------------------------------------
// Helper: build a Fory instance with all envelope types registered for u32 T.
// ---------------------------------------------------------------------------

fn build_fory_u32() -> Fory {
    let mut fory = Fory::default();
    fory.register::<ForyTraceContext>(2).unwrap();
    fory.register::<ForyServerError>(3).unwrap();
    fory.register::<ForyResult<u32>>(4).unwrap();
    fory.register::<ForyRequest<u32>>(5).unwrap();
    fory.register::<ForyResponse<u32>>(6).unwrap();
    fory.register::<ForyClientMessage<u32>>(7).unwrap();
    fory
}

fn build_fory_string() -> Fory {
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
// ClientMessage::Request round-trip
// ---------------------------------------------------------------------------

#[test]
fn client_message_request_round_trip() {
    let fory = build_fory_string();

    // Build a native ClientMessage::Request value.
    let mut ctx = context::current();
    ctx.deadline = Instant::now() + Duration::from_secs(60);
    ctx.trace_context = trace::Context {
        trace_id: TraceId::from(0x1234_5678_9abc_def0_1234_5678_9abc_def0_u128),
        span_id: SpanId::from(0xdead_beef_cafe_babe_u64),
        sampling_decision: SamplingDecision::Sampled,
    };

    let native: ClientMessage<String> = ClientMessage::Request(Request {
        context: ctx,
        id: 42,
        message: "hello".to_string(),
    });

    // native → wrapper → bytes → wrapper → native
    let wrapper: ForyClientMessage<String> = (&native).into();
    let bytes = fory.serialize(&wrapper).unwrap();
    let decoded_wrapper: ForyClientMessage<String> = fory.deserialize(&bytes).unwrap();
    let native_again: ClientMessage<String> = decoded_wrapper.into();

    // Verify the user-payload survives.
    match (&native, &native_again) {
        (ClientMessage::Request(a), ClientMessage::Request(b)) => {
            assert_eq!(a.id, b.id);
            assert_eq!(a.message, b.message);
            assert_eq!(
                u128::from(a.context.trace_context.trace_id),
                u128::from(b.context.trace_context.trace_id)
            );
            assert_eq!(
                u64::from(a.context.trace_context.span_id),
                u64::from(b.context.trace_context.span_id)
            );
            assert_eq!(
                a.context.trace_context.sampling_decision,
                b.context.trace_context.sampling_decision
            );
            // Deadline is encoded as relative nanos and re-anchored at decode time.
            // The reconstructed deadline should be within 1 second of the original
            // (allowing for serialize/deserialize latency).
            let drift = a
                .context
                .deadline
                .saturating_duration_since(b.context.deadline)
                .max(b.context.deadline.saturating_duration_since(a.context.deadline));
            assert!(
                drift < Duration::from_secs(1),
                "deadline drift > 1s: {:?}",
                drift
            );
        }
        _ => panic!("expected Request, got Cancel after round-trip"),
    }
}

// ---------------------------------------------------------------------------
// ClientMessage::Cancel round-trip
// ---------------------------------------------------------------------------

#[test]
fn client_message_cancel_round_trip() {
    let fory = build_fory_u32();

    let trace_ctx = trace::Context {
        trace_id: TraceId::from(99_u128),
        span_id: SpanId::from(100_u64),
        sampling_decision: SamplingDecision::Unsampled,
    };
    let native: ClientMessage<u32> = ClientMessage::Cancel {
        trace_context: trace_ctx,
        request_id: 1234,
    };

    let wrapper: ForyClientMessage<u32> = (&native).into();
    let bytes = fory.serialize(&wrapper).unwrap();
    let decoded: ForyClientMessage<u32> = fory.deserialize(&bytes).unwrap();
    let native_again: ClientMessage<u32> = decoded.into();

    match native_again {
        ClientMessage::Cancel {
            trace_context,
            request_id,
        } => {
            assert_eq!(request_id, 1234);
            assert_eq!(u128::from(trace_context.trace_id), 99_u128);
            assert_eq!(u64::from(trace_context.span_id), 100_u64);
            assert_eq!(
                trace_context.sampling_decision,
                SamplingDecision::Unsampled
            );
        }
        _ => panic!("expected Cancel"),
    }
}

// ---------------------------------------------------------------------------
// Response::Ok round-trip
// ---------------------------------------------------------------------------

#[test]
fn response_ok_round_trip() {
    let fory = build_fory_string();

    let native: Response<String> = Response {
        request_id: 7,
        message: Ok("ok-payload".to_string()),
    };

    let wrapper: ForyResponse<String> = (&native).into();
    let bytes = fory.serialize(&wrapper).unwrap();
    let decoded: ForyResponse<String> = fory.deserialize(&bytes).unwrap();
    let native_again: Response<String> = decoded.into();

    assert_eq!(native_again.request_id, 7);
    assert_eq!(native_again.message.unwrap(), "ok-payload");
}

// ---------------------------------------------------------------------------
// Response::Err round-trip
// ---------------------------------------------------------------------------

#[test]
fn response_error_round_trip() {
    let fory = build_fory_u32();

    let native: Response<u32> = Response {
        request_id: 8,
        message: Err(ServerError::new(
            std::io::ErrorKind::NotFound,
            "missing".to_string(),
        )),
    };

    let wrapper: ForyResponse<u32> = (&native).into();
    let bytes = fory.serialize(&wrapper).unwrap();
    let decoded: ForyResponse<u32> = fory.deserialize(&bytes).unwrap();
    let native_again: Response<u32> = decoded.into();

    assert_eq!(native_again.request_id, 8);
    let err = native_again.message.expect_err("should be Err");
    assert_eq!(err.kind, std::io::ErrorKind::NotFound);
    assert_eq!(err.detail, "missing");
}

// ---------------------------------------------------------------------------
// Request id + message + trace context field completeness check
// ---------------------------------------------------------------------------

#[test]
fn request_all_fields_survive() {
    let fory = build_fory_u32();

    let mut ctx = context::current();
    ctx.deadline = Instant::now() + Duration::from_secs(30);
    ctx.trace_context = trace::Context {
        trace_id: TraceId::from(0xABCD_EF01_2345_6789_u128),
        span_id: SpanId::from(0xFEDC_BA98_u64),
        sampling_decision: SamplingDecision::Sampled,
    };

    let native: ClientMessage<u32> = ClientMessage::Request(Request {
        context: ctx,
        id: 99,
        message: 42_u32,
    });

    let wrapper: ForyClientMessage<u32> = (&native).into();
    let bytes = fory.serialize(&wrapper).unwrap();
    let decoded: ForyClientMessage<u32> = fory.deserialize(&bytes).unwrap();
    let native_again: ClientMessage<u32> = decoded.into();

    match native_again {
        ClientMessage::Request(req) => {
            assert_eq!(req.id, 99);
            assert_eq!(req.message, 42_u32);
            assert_eq!(
                u128::from(req.context.trace_context.trace_id),
                0xABCD_EF01_2345_6789_u128
            );
            assert_eq!(
                u64::from(req.context.trace_context.span_id),
                0xFEDC_BA98_u64
            );
            assert_eq!(
                req.context.trace_context.sampling_decision,
                SamplingDecision::Sampled
            );
            // Deadline should still be in the future and within 30s.
            let remaining = req.context.deadline.saturating_duration_since(Instant::now());
            assert!(
                remaining > Duration::from_secs(28),
                "remaining deadline too short: {remaining:?}"
            );
        }
        _ => panic!("expected Request variant"),
    }
}
