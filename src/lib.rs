// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

//! An RPC library for Rust.
//!
//! Example usage:
//!
//! ```
//! #![feature(conservative_impl_trait)]
//! #[macro_use]
//! extern crate tarpc;
//! extern crate futures;
//!
//! use tarpc::Connect;
//! use futures::Future;
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
//!     fn hello(&self, s: String) -> tarpc::Result<String> {
//!         Ok(format!("Hello, {}!", s))
//!     }
//!
//!     fn add(&self, x: i32, y: i32) -> tarpc::Result<i32> {
//!         Ok(x + y)
//!     }
//! }
//!
//! fn main() {
//!     let serve_handle = Server.listen("localhost:0").unwrap();
//!     let client = SyncClient::connect(serve_handle.local_addr()).wait().unwrap();
//!     assert_eq!(3, client.add(&1, &2).unwrap());
//!     assert_eq!("Hello, Mom!", client.hello(&"Mom".to_string()).unwrap());
//! }
//! ```
//!
#![deny(missing_docs)]
#![feature(custom_derive, plugin, question_mark, conservative_impl_trait)]
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
extern crate take;

#[doc(hidden)]
pub extern crate futures;
#[doc(hidden)]
pub extern crate futures_cpupool;
#[doc(hidden)]
pub extern crate serde;
#[doc(hidden)]
pub extern crate tokio_core;
#[doc(hidden)]
pub extern crate tokio_proto;
#[doc(hidden)]
pub extern crate tokio_service;

pub use client::Connect;
pub use errors::{Error, RpcError, RpcErrorCode};

#[doc(hidden)]
pub use client::Client;
#[doc(hidden)]
pub use protocol::{Packet, deserialize};
#[doc(hidden)]
pub use server::{SerializeFuture, SerializedReply, Server, serialize_reply};

/// Provides the macro used for constructing rpc services and client stubs.
#[macro_use]
mod macros;
/// Provides the base client stubs used by the service macro.
mod client;
/// Provides the base server boilerplate used by service implementations.
mod server;
/// Provides the tarpc client and server, which implements the tarpc protocol.
/// The protocol is defined by the implementation.
mod protocol;
/// Provides a few different error types.
mod errors;

/// Return type of rpc calls: either the successful return value, or a client error.
pub type Result<T> = ::std::result::Result<T, Error>;
/// Return type from server to client. Converted into ```Result<T>``` before reaching the user.
pub type Future<T> = futures::BoxFuture<T, Error>;
