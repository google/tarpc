// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

//! tarpc is an RPC framework for rust with a focus on ease of use. Defining a
//! service can be done in just a few lines of code, and most of the boilerplate of
//! writing a server is taken care of for you.
//!
//! ## What is an RPC framework?
//! "RPC" stands for "Remote Procedure Call," a function call where the work of
//! producing the return value is being done somewhere else. When an rpc function is
//! invoked, behind the scenes the function contacts some other process somewhere
//! and asks them to evaluate the function instead. The original function then
//! returns the value produced by the other process.
//!
//! RPC frameworks are a fundamental building block of most microservices-oriented
//! architectures. Two well-known ones are [gRPC](http://www.grpc.io) and
//! [Cap'n Proto](https://capnproto.org/).
//!
//! tarpc differentiates itself from other RPC frameworks by defining the schema in code,
//! rather than in a separate language such as .proto. This means there's no separate compilation
//! process, and no cognitive context switching between different languages. Additionally, it
//! works with the community-backed library serde: any serde-serializable type can be used as
//! arguments to tarpc fns.
//!
//! Example usage:
//!
//! ```
//! // required by `FutureClient` (not used in this example)
//! #![feature(conservative_impl_trait, plugin)]
//! #![plugin(snake_to_camel)]
//!
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
#![feature(plugin, question_mark, conservative_impl_trait, never_type, rustc_macro)]
#![plugin(snake_to_camel)]

extern crate bincode;
extern crate byteorder;
extern crate bytes;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
#[macro_use]
extern crate serde_derive;
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
pub use errors::{Error, SerializableError};

#[doc(hidden)]
pub use client::Client;
#[doc(hidden)]
pub use client::future::ClientFuture;
#[doc(hidden)]
pub use errors::{WireError};
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
