// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! An RPC library for Rust.
//!
//! Example usage:
//!
//! ```
//! # #![feature(custom_derive, plugin)]
//! # #![plugin(serde_macros)]
//! # #[macro_use] extern crate tarpc;
//! # extern crate serde;
//! rpc! {
//!     mod my_server {
//!         service {
//!             rpc hello(name: String) -> String;
//!             rpc add(x: i32, y: i32) -> i32;
//!         }
//!     }
//! }
//!
//! use self::my_server::*;
//! use std::time::Duration;
//!
//! struct Server;
//! impl my_server::Service for Server {
//!     fn hello(&self, s: String) -> String {
//!         format!("Hello, {}!", s)
//!     }
//!     fn add(&self, x: i32, y: i32) -> i32 {
//!         x + y
//!     }
//! }
//!
//! fn main() {
//!     let addr = "127.0.0.1:9000";
//!     let shutdown = my_server::serve(addr,
//!                                     Server,
//!                                     Some(Duration::from_secs(30)))
//!                               .unwrap();
//!     let client = Client::new(addr, None).unwrap();
//!     assert_eq!(3, client.add(1, 2).unwrap());
//!     assert_eq!("Hello, Mom!".to_string(),
//!                client.hello("Mom".to_string()).unwrap());
//!     drop(client);
//!     shutdown.shutdown();
//! }
//! ```

#![deny(missing_docs)]
#![feature(custom_derive, plugin, test)]
#![plugin(serde_macros)]

extern crate serde;
extern crate bincode;
#[macro_use]
extern crate log;
extern crate crossbeam;
extern crate test;

/// Provides the tarpc client and server, which implements the tarpc protocol.
/// The protocol is defined by the implementation.
pub mod protocol;

/// Provides the macro used for constructing rpc services and client stubs.
pub mod macros;

pub use protocol::{Error, Result, ServeHandle};
