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

#![deny(missing_docs, missing_debug_implementations)]
#![feature(
    futures_api,
    pin,
    arbitrary_self_types,
    await_macro,
    async_await
)]
#![cfg_attr(test, feature(plugin))]
#![cfg_attr(test, plugin(tarpc_plugins))]

extern crate byteorder;
extern crate bytes;
#[macro_use]
extern crate log;
extern crate net2;
extern crate num_cpus;
extern crate thread_pool;
extern crate tokio_codec;
extern crate tokio_io;

#[doc(hidden)]
pub extern crate bincode;
#[doc(hidden)]
pub extern crate rpc;
#[doc(hidden)]
#[macro_use]
pub extern crate futures;
#[doc(hidden)]
pub extern crate serde;
#[doc(hidden)]
#[macro_use]
pub extern crate serde_derive;
#[doc(hidden)]
pub extern crate tokio_core;
#[doc(hidden)]
pub extern crate tokio_proto;
#[doc(hidden)]
pub extern crate tokio_service;

/// Provides the macro used for constructing rpc services and client stubs.
#[macro_use]
mod macros;
