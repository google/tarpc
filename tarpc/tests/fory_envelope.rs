// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Round-trip test scaffold for ForyObject derives on tarpc envelope types.
//!
//! # Status: BLOCKED — see module-level docs.
//!
//! ## Inventory of envelope types and blockers
//!
//! ### Envelope types in `tarpc/src/lib.rs`
//!
//! | Type | Generic | Fields |
//! |------|---------|--------|
//! | `ClientMessage<T>` | `T` | `Request(Request<T>)`, `Cancel { trace_context: trace::Context, request_id: u64 }` |
//! | `Request<T>` | `T` | `context: context::Context`, `id: u64`, `message: T` |
//! | `Response<T>` | `T` | `request_id: u64`, `message: Result<T, ServerError>` |
//! | `ServerError` | none | `kind: io::ErrorKind`, `detail: String` |
//!
//! ### Sub-types and their serializability
//!
//! | Type | Fory-serializable? | Reason |
//! |------|--------------------|--------|
//! | `trace::TraceId(u128)` | YES | u128 is serializable |
//! | `trace::SpanId(u64)` | YES | u64 is serializable |
//! | `trace::SamplingDecision` | YES | C-style enum, u8-repr |
//! | `trace::Context` | YES — derivable | all fields are serializable |
//! | `context::Context` | **NO** | contains `deadline: std::time::Instant` |
//! | `ServerError` | **NO** | contains `kind: io::ErrorKind` |
//! | `Result<T, ServerError>` | **NO** | no `impl Serializer for Result<T, E>` in fory-core |
//!
//! ### Compiler error (confirming the wall)
//!
//! Attempting `#[cfg_attr(feature = "fory", derive(fory::ForyObject))]` on `Request<T>` produces:
//! ```text
//! error[E0277]: the trait bound `Instant: fory_core::Serializer` is not satisfied
//! ```
//!
//! Attempting it on `Response<T>` produces:
//! ```text
//! error[E0277]: the trait bound `Result<T, ServerError>: fory_core::Serializer` is not satisfied
//! ```
//!
//! ## Cargo.toml changes required (not yet applied — Task 3.2 follow-up)
//!
//! ```toml
//! # in [dependencies]:
//! fory-core = { version = "0.17", optional = true }
//!
//! # in [features]:
//! fory = ["dep:fory", "dep:fory-core"]
//!
//! # in [dev-dependencies]:
//! fory-core = { version = "0.17" }
//! ```
//!
//! The `fory-derive` macro emits `fory_core::` paths in generated code.
//! In Rust 2024 edition, transitive crates are not in scope; `fory-core` must be
//! a direct dependency of any crate using `#[derive(ForyObject)]`.
//!
//! ## Resolution path (Task 3.4)
//!
//! Define parallel fory-specific envelope types in `tarpc/src/serde_transport/fory.rs`:
//!
//! ```rust
//! #[derive(fory::ForyObject, Debug, PartialEq)]
//! pub struct ForyTraceContext { pub trace_id: u128, pub span_id: u64, pub sampling: u8 }
//!
//! #[derive(fory::ForyObject, Debug, PartialEq)]
//! pub struct ForyServerError { pub kind: u32, pub detail: String }
//!
//! #[derive(fory::ForyObject, Debug, PartialEq)]
//! pub enum ForyResult<T> { Ok(T), Err(ForyServerError) }
//!
//! #[derive(fory::ForyObject, Debug, PartialEq)]
//! pub struct ForyRequest<T> {
//!     pub id: u64,
//!     pub trace: ForyTraceContext,
//!     pub deadline_ns: u64,   // Instant replaced by relative nanoseconds from now
//!     pub message: T,
//! }
//!
//! #[derive(fory::ForyObject, Debug, PartialEq)]
//! pub struct ForyResponse<T> {
//!     pub request_id: u64,
//!     pub message: ForyResult<T>,
//! }
//!
//! #[derive(fory::ForyObject, Debug, PartialEq)]
//! pub enum ForyClientMessage<T> {
//!     Request(ForyRequest<T>),
//!     Cancel { trace: ForyTraceContext, request_id: u64 },
//! }
//! ```
//!
//! The tarpc transport adapter (Task 3.4) converts these to/from native tarpc types
//! on the wire boundary. `deadline_ns` is re-anchored to a local `Instant::now() + duration`
//! on the receiving end.
//!
//! Once Task 3.4 is complete and Cargo.toml is updated, this test file should be
//! expanded with full round-trip assertions for all ForyXxx envelope types.

#![cfg(feature = "fory")]

/// Feature-flag smoke test: verifies that `use fory::Fory` is accessible
/// when the `fory` feature is enabled. Does not exercise envelope types yet.
/// Full envelope round-trip tests are deferred to Task 3.4.
#[test]
fn fory_feature_available() {
    // If this compiles, the `fory` feature flag wiring is correct.
    let _fory = fory::Fory::default();
}
