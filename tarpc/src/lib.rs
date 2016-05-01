// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

//! An RPC library for Rust.
//!
//! Example usage:
//!
//! ```
//! #[macro_use] extern crate tarpc;
//! mod my_server {
//!     service! {
//!         rpc hello(name: String) -> String;
//!         rpc add(x: i32, y: i32) -> i32;
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
//!     let serve_handle = Server.spawn("localhost:0").unwrap();
//!     let client = Client::new(serve_handle.dialer()).unwrap();
//!     assert_eq!(3, client.add(1, 2).unwrap());
//!     assert_eq!("Hello, Mom!".to_string(),
//!                client.hello("Mom".to_string()).unwrap());
//!     drop(client);
//!     serve_handle.shutdown();
//! }
//! ```

#![feature(question_mark)]

extern crate serde;
extern crate bincode;
extern crate byteorder;
#[macro_use]
extern crate log;
extern crate scoped_pool;
extern crate unix_socket;
extern crate mio;
#[macro_use]
extern crate quick_error;

macro_rules! pos {
    () => (concat!(file!(), ":", line!()))
}

/// Provides the tarpc client and server, which implements the tarpc protocol.
/// The protocol is defined by the implementation.
pub mod protocol;

/// Provides the macro used for constructing rpc services and client stubs.
pub mod macros;

/// Provides transport traits and implementations.
pub mod transport;

pub use protocol::{Config, Error, Result, ServeHandle};
