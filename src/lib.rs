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
//! impl my_server::BlockingService for Server {
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
//!     let client = BlockingClient::spawn(serve_handle.local_addr()).unwrap();
//!     assert_eq!(3, client.add(&1, &2).unwrap());
//!     assert_eq!("Hello, Mom!".to_string(),
//!                client.hello(&"Mom".to_string()).unwrap());
//!     client.shutdown().unwrap();
//!     serve_handle.shutdown();
//! }
//! ```

#![deny(missing_docs)]

extern crate byteorder;
extern crate scoped_pool;
extern crate unix_socket;
#[macro_use]
extern crate quick_error;

macro_rules! pos {
    () => (concat!(file!(), ":", line!()))
}

use mio::NotifyError;
use std::io;
use std::sync::mpsc;

quick_error! {
    /// All errors that can occur during the use of tarpc.
    #[derive(Debug)]
    pub enum Error {
        /// No address found for the specified address.
        /// Depending on the outcome of address resolution, `ToSocketAddrs` may not yield any
        /// values, which will propagate as this variant.
        NoAddressFound {}
        /// The client or service hung up.
        ConnectionBroken {}
        /// Any IO error other than ConnectionBroken.
        Io(err: io::Error) {
            from()
            description(err.description())
        }
        /// Error in receiving a value, typically from an event loop that may have shutdown.
        Rx(err: mpsc::RecvError) {
            from()
            description(err.description())
        }
        /// Error in serializing, either on client or server.
        Deserialize(err: bincode::serde::DeserializeError) {
            from()
            description(err.description())
        }
        /// Error in deserializing, either on client or server.
        Serialize(err: bincode::serde::SerializeError) {
            from()
            description(err.description())
        }
        /// Error in sending a notification to the client event loop.
        ClientNotify(err: NotifyError<protocol::client::Action>) {
            from()
            description(err.description())
        }
        /// Error in sending a notification to the server event loop.
        ServerNotify(err: NotifyError<protocol::server::Action>) {
            from()
            description(err.description())
        }
    }
}

/// Return type of rpc calls: either the successful return value, or a client error.
pub type Result<T> = ::std::result::Result<T, Error>;

/// Re-exported for use by macros.
pub extern crate serde;
/// Re-exported for use by macros.
pub extern crate bincode;
/// Re-exported for use by macros.
pub extern crate mio;
/// Re-exported for use by macros.
#[macro_use]
pub extern crate log;

/// Provides the tarpc client and server, which implements the tarpc protocol.
/// The protocol is defined by the implementation.
pub mod protocol;

/// Provides the macro used for constructing rpc services and client stubs.
pub mod macros;
