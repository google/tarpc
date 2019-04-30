// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#![doc(include = "../README.md")]
#![deny(missing_docs, missing_debug_implementations)]
#![feature(async_await, external_doc)]
#![cfg_attr(
    test,
    feature(await_macro, proc_macro_hygiene, arbitrary_self_types)
)]

#[doc(hidden)]
pub use futures;
pub use rpc::*;
#[cfg(feature = "serde")]
#[doc(hidden)]
pub use serde;
#[doc(hidden)]
pub use tarpc_plugins::*;

/// Provides the macro used for constructing rpc services and client stubs.
#[macro_use]
mod macros;
