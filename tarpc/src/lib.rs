// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#![doc(include = "../README.md")]
#![deny(missing_docs, missing_debug_implementations)]
#![feature(async_await, external_doc)]
#![cfg_attr(test, feature(proc_macro_hygiene))]

pub use rpc::*;
/// The main macro that creates RPC services.
///
/// Rpc methods are specified, mirroring trait syntax:
///
/// ```
/// # #![feature(async_await, proc_macro_hygiene)]
/// # fn main() {}
/// #[tarpc::service]
/// trait Service {
/// /// Say hello
/// async fn hello(name: String) -> String;
/// }
/// ```
///
/// Attributes can be attached to each rpc. These attributes
/// will then be attached to the generated service traits'
/// corresponding `fn`s, as well as to the client stubs' RPCs.
///
/// The following items are expanded in the enclosing module:
///
/// * `trait Service` -- defines the RPC service.
///   * `fn serve` -- turns a service impl into a request handler.
/// * `Client` -- a client stub with a fn for each RPC.
///   * `fn new_stub` -- creates a new Client stub.
pub use tarpc_plugins::service;
