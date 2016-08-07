// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

//! An RPC library for Rust.
//!
//! Example usage:
//!
//! ```
//! #![feature(default_type_parameter_fallback)]
//! #[macro_use]
//! extern crate tarpc;
//!
//! use tarpc::{Connect, RpcResult};
//!
//! service! {
//!     rpc hello(name: String) -> String;
//!     rpc add(x: i32, y: i32) -> i32;
//! }
//!
//! #[derive(Clone, Copy)]
//! struct Server;
//!
//! impl SyncService for Server {
//!     fn hello(&self, s: String) -> RpcResult<String> {
//!         Ok(format!("Hello, {}!", s))
//!     }
//!
//!     fn add(&self, x: i32, y: i32) -> RpcResult<i32> {
//!         Ok(x + y)
//!     }
//! }
//!
//! fn main() {
//!     let serve_handle = Server.listen("localhost:0").unwrap();
//!     let client = SyncClient::connect(serve_handle.local_addr()).unwrap();
//!     assert_eq!(3, client.add(&1, &2).unwrap());
//!     assert_eq!("Hello, Mom!", client.hello(&"Mom".to_string()).unwrap());
//! }
//! ```
//!
#![deny(missing_docs)]
#![feature(custom_derive, plugin, default_type_parameter_fallback, question_mark)]
#![plugin(serde_macros)]

extern crate bincode;
extern crate byteorder;
extern crate bytes;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
#[macro_use]
extern crate quick_error;

/// Re-exported for use by macros.
pub extern crate tokio;
/// Re-exported for use by macros.
pub extern crate futures;
/// Re-exported for use by macros.
pub extern crate serde;

/// Adds a convenience method to Futures to block until the result is available.
pub trait Await: futures::Future + Sized {
    /// Blocks until the pending result is available.
    fn await(self) -> ::std::result::Result<Self::Item, Self::Error> {
        use futures::Future;
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel();

        self.then(move |res| {
                tx.send(res).unwrap();
                Ok::<(), ()>(())
            })
            .forget();

        rx.recv().unwrap()
    }
}

impl<T: futures::Future> Await for T {}

pub use client::{Connect, Reply};
pub use errors::{CanonicalRpcError, CanonicalRpcErrorCode, Error, Future, Result, RpcError,
                 RpcErrorCode, RpcResult};

#[doc(hidden)]
pub use client::{Client, Handle as ClientHandle};
#[doc(hidden)]
pub use protocol::{Packet, deserialize, serialize};
#[doc(hidden)]
pub use server::{Server, reply, serialize_reply};

/// Provides the base client stubs used by the service macro.
mod client;
/// Provides the base server boilerplate used by service implementations.
mod server;
/// Provides the tarpc client and server, which implements the tarpc protocol.
/// The protocol is defined by the implementation.
mod protocol;
/// Provides the macro used for constructing rpc services and client stubs.
mod macros;
/// Provides a few different error types.
mod errors;
