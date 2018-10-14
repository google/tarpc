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
//! ## Example
//!
//! Here's a small service.
//!
//! ```rust
//! #![feature(plugin, futures_api, pin, arbitrary_self_types, await_macro, async_await, existential_type)]
//! #![plugin(tarpc_plugins)]
//!
//!
//! use futures::{
//!     compat::TokioDefaultSpawner,
//!     future::{self, Ready},
//!     prelude::*,
//! };
//! use tarpc::{
//!     client, context,
//!     server::{self, Handler, Server},
//! };
//! use std::io;
//!
//! // This is the service definition. It looks a lot like a trait definition.
//! // It defines one RPC, hello, which takes one arg, name, and returns a String.
//! tarpc::service! {
//!     /// Returns a greeting for name.
//!     rpc hello(name: String) -> String;
//! }
//!
//! // This is the type that implements the generated Service trait. It is the business logic
//! // and is used to start the server.
//! #[derive(Clone)]
//! struct HelloServer;
//!
//! impl Service for HelloServer {
//!     // Each defined rpc generates two items in the trait, a fn that serves the RPC, and
//!     // an associated type representing the future output by the fn.
//!
//!     type HelloFut = Ready<String>;
//!
//!     fn hello(&self, _: context::Context, name: String) -> Self::HelloFut {
//!         future::ready(format!("Hello, {}!", name))
//!     }
//! }
//!
//! async fn run() -> io::Result<()> {
//!     // bincode_transport is provided by the associated crate bincode-transport. It makes it easy
//!     // to start up a serde-powered bincode serialization strategy over TCP.
//!     let transport = bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?;
//!     let addr = transport.local_addr();
//!
//!     // The server is configured with the defaults.
//!     let server = Server::new(server::Config::default())
//!         // Server can listen on any type that implements the Transport trait.
//!         .incoming(transport)
//!         // Close the stream after the client connects
//!         .take(1)
//!         // serve is generated by the service! macro. It takes as input any type implementing
//!         // the generated Service trait.
//!         .respond_with(serve(HelloServer));
//!
//!     tokio_executor::spawn(server.unit_error().boxed().compat());
//!
//!     let transport = await!(bincode_transport::connect(&addr))?;
//!
//!     // new_stub is generated by the service! macro. Like Server, it takes a config and any
//!     // Transport as input, and returns a Client, also generated by the macro.
//!     // by the service mcro.
//!     let mut client = await!(new_stub(client::Config::default(), transport))?;
//!
//!     // The client has an RPC method for each RPC defined in service!. It takes the same args
//!     // as defined, with the addition of a Context, which is always the first arg. The Context
//!     // specifies a deadline and trace information which can be helpful in debugging requests.
//!     let hello = await!(client.hello(context::current(), "Stim".to_string()))?;
//!
//!     println!("{}", hello);
//!
//!     Ok(())
//! }
//!
//! fn main() {
//!     tarpc::init(TokioDefaultSpawner);
//!     tokio::run(run()
//!             .map_err(|e| eprintln!("Oh no: {}", e))
//!             .boxed()
//!             .compat(),
//!     );
//! }
//! ```

#![deny(missing_docs, missing_debug_implementations)]
#![feature(
    futures_api,
    pin,
    arbitrary_self_types,
    await_macro,
    async_await,
    decl_macro,
)]
#![cfg_attr(test, feature(plugin, existential_type))]
#![cfg_attr(test, plugin(tarpc_plugins))]

#[doc(hidden)]
pub use futures;
pub use rpc::*;
#[cfg(feature = "serde")]
#[doc(hidden)]
pub use serde;

/// Provides the macro used for constructing rpc services and client stubs.
#[macro_use]
mod macros;
