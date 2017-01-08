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
#![cfg_attr(feature = "tls", doc = " ```ignore")]
#![cfg_attr(not(feature = "tls"), doc = " ```")]
//! // required by `FutureClient` (not used in this example)
//! #![feature(conservative_impl_trait, plugin)]
//! #![plugin(tarpc_plugins)]
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
//!     fn hello(&mut self, name: String) -> Result<String, Never> {
//!         Ok(format!("Hello, {}!", name))
//!     }
//! }
//!
//! fn main() {
//!     let addr = "localhost:10000";
//!     let _server = HelloServer.listen(addr);
//!     let mut client = SyncClient::connect(addr).unwrap();
//!     println!("{}", client.hello("Mom".to_string()).unwrap());
//! }
//! ```
//!
//! Example usage with TLS:
//!
#![cfg_attr(feature = "tls", doc = " ```ignore,rust,no_run")]
#![cfg_attr(not(feature = "tls"), doc = " ```ignore")]
//! // required by `FutureClient` (not used in this example)
//! #![feature(conservative_impl_trait, plugin)]
//! #![plugin(tarpc_plugins)]
//!
//! #[macro_use]
//! extern crate tarpc;
//!
//! use tarpc::sync::Connect;
//! use tarpc::util::Never;
//! use tarpc::TlsClientContext;
//! use tarpc::native_tls::{Pkcs12, TlsAcceptor, TlsConnector};
//!
//! service! {
//!     rpc hello(name: String) -> String;
//! }
//!
//! #[derive(Clone)]
//! struct HelloServer;
//!
//! impl SyncService for HelloServer {
//!     fn hello(&mut self, name: String) -> Result<String, Never> {
//!         Ok(format!("Hello, {}!", name))
//!     }
//! }
//!
//! fn tls_context() -> (TlsAcceptor, TlsClientContext) {
//!      let buf = include_bytes!("test/identity.p12");
//!      let pkcs12 = Pkcs12::from_der(buf, "password").unwrap();
//!      let acceptor = TlsAcceptor::builder(pkcs12).unwrap().build().unwrap();
//!      let client_cx = TlsClientContext::try_new("foobar.com").unwrap();
//!      (acceptor, client_cx)
//! }
//!
//! fn main() {
//!     let addr = "localhost:10000";
//!     let (acceptor, client_cx) = tls_context();
//!     let _server = HelloServer.listen(addr, acceptor);
//!     let mut client = SyncClient::connect(addr, client_cx).unwrap();
//!     println!("{}", client.hello("Mom".to_string()).unwrap());
//! }
//! ```
//!
#![deny(missing_docs)]
#![feature(plugin, conservative_impl_trait, never_type, unboxed_closures, fn_traits, specialization)]
#![plugin(tarpc_plugins)]

extern crate byteorder;
extern crate bytes;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
extern crate net2;
#[macro_use]
extern crate serde_derive;
extern crate take;
#[macro_use]
extern crate cfg_if;

#[doc(hidden)]
pub extern crate bincode;
#[doc(hidden)]
pub extern crate futures;
#[doc(hidden)]
pub extern crate serde;
#[doc(hidden)]
pub extern crate tokio_core;
#[doc(hidden)]
pub extern crate tokio_proto;
#[doc(hidden)]
pub extern crate tokio_service;

#[doc(hidden)]
pub use client::Client;
pub use client::Config as ClientConfig;
#[doc(hidden)]
pub use client::future::{ConnectFuture, ConnectWithFuture};
pub use errors::{Error, SerializableError};
#[doc(hidden)]
pub use errors::WireError;
#[doc(hidden)]
pub use server::{ListenFuture, Response, Listen};
pub use server::Config as ServerConfig;
/// TcpStream
pub type Tcp = tokio_core::net::TcpStream;

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
/// Provides implementations of `ClientProto` and `ServerProto` that implement the tarpc protocol.
/// The tarpc protocol is a length-delimited, bincode-serialized payload.
mod protocol;
/// Provides a few different error types.
mod errors;

cfg_if! {
    if #[cfg(feature = "tls")] {
        extern crate tokio_tls;
        pub extern crate native_tls;
        /// TlsStream<TcpStream>
        pub type Tls = tokio_tls::TlsStream<::tokio_core::net::TcpStream>;

        pub use client::tls::TlsClientContext;
    } else {}
}

/// Utility specific to synchronous implementation.
pub mod sync {
    pub use client::sync::*;
}

/// Utility specific to futures implementation.
pub mod future {
    pub use client::future::*;
    use tokio_core::reactor::Remote;
    use util;

    lazy_static! {
        /// The `Remote` for the default reactor core.
        pub static ref REMOTE: Remote = {
            util::spawn_core()
        };
    }
}
