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
//! 
//! extern crate futures;
//! #[macro_use]
//! extern crate tarpc;
//! 
//! use tarpc::sync::Connect;
//! use tarpc::util::Never;
//! 
//! service! {
//!     rpc hello(name: String) -> String;
//! }
//! 
//! #[derive(Clone)]
//! struct HelloServer;
//! 
//! impl SyncService for HelloServer {
//!     fn hello(&self, name: String) -> Result<String, Never> {
//!         Ok(format!("Hello, {}!", name))
//!     }
//! }
//! 
//! fn main() {
//!     let addr = "localhost:10000";
//!     let _server = HelloServer.listen(addr).unwrap();
//!     let client = SyncClient::connect(addr).unwrap();
//!     println!("{}", client.hello(&"Mom".to_string()).unwrap());
//! }
//! ```
//!
#![deny(missing_docs)]
#![feature(custom_derive, plugin, question_mark, conservative_impl_trait, never_type)]
#![plugin(serde_macros)]

extern crate bincode;
extern crate byteorder;
extern crate bytes;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
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

pub use client::{sync, future};
pub use errors::Error;
pub use protocol::Spawn;

#[doc(hidden)]
pub use client::Client;
#[doc(hidden)]
pub use errors::{SerializableError, WireError};
#[doc(hidden)]
pub use protocol::{Packet, deserialize};
#[doc(hidden)]
pub use server::{SerializeFuture, SerializedReply, listen, serialize_reply};

/// Provides some utility error types, as well as a trait for spawning futures on the default event
/// loop.
pub mod util;

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
pub type Result<T, E> = ::std::result::Result<T, Error<E>>;
/// Return type from server to client. Converted into ```Result<T>``` before reaching the user.
pub type Future<T, E> = futures::BoxFuture<T, E>;
