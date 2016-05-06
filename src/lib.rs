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
//!     let client = Client::spawn(serve_handle.local_addr).unwrap();
//!     assert_eq!(3, client.add(&1, &2).unwrap());
//!     assert_eq!("Hello, Mom!".to_string(),
//!                client.hello(&"Mom".to_string()).unwrap());
//!     client.shutdown().unwrap();
//!     serve_handle.shutdown();
//! }
//! ```

#![feature(question_mark)]
//#![deny(missing_docs)]

extern crate byteorder;
#[macro_use]
extern crate log;
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
        /// The client or service hung up.
        ConnectionBroken {}
        /// Any IO error other than ConnectionBroken.
        Io(err: io::Error) {
            from()
            description(err.description())
        }
        Rx(err: mpsc::RecvError) {
            from()
            description(err.description())
        }
        /// Serialization error.
        Deserialize(err: bincode::serde::DeserializeError) {
            from()
            description(err.description())
        }
        Serialize(err: bincode::serde::SerializeError) {
            from()
            description(err.description())
        }
        DeregisterClient(err: NotifyError<()>) {
            from(DeregisterClientError)
            description(err.description())
        }
        RegisterClient(err: NotifyError<()>) {
            from(RegisterClientError)
            description(err.description())
        }
        DeregisterServer(err: NotifyError<()>) {
            from(DeregisterServerError)
            description(err.description())
        }
        RegisterServer(err: NotifyError<()>) {
            from(RegisterServerError)
            description(err.description())
        }
        Rpc(err: NotifyError<()>) {
            from(RpcError)
            description(err.description())
        }
        ShutdownClient(err: NotifyError<()>) {
            from(ShutdownClientError)
            description(err.description())
        }
        ShutdownServer(err: NotifyError<()>) {
            from(ShutdownServerError)
            description(err.description())
        }
        NoAddressFound {}
    }
}

struct RegisterServerError(NotifyError<protocol::server::Action>);
struct DeregisterServerError(NotifyError<protocol::server::Action>);
struct ShutdownServerError(NotifyError<protocol::server::Action>);
struct RegisterClientError(NotifyError<protocol::client::Action>);
struct DeregisterClientError(NotifyError<protocol::client::Action>);
struct ShutdownClientError(NotifyError<protocol::client::Action>);
struct RpcError(NotifyError<protocol::client::Action>);

macro_rules! from_err {
    ($from:ty, $to:expr) => {
        impl ::std::convert::From<$from> for Error {
            fn from(e: $from) -> Self {
                $to(discard_inner(e.0))
            }
        }
    }
}

from_err!(RegisterServerError, Error::RegisterServer);
from_err!(DeregisterServerError, Error::DeregisterServer);
from_err!(RegisterClientError, Error::RegisterClient);
from_err!(DeregisterClientError, Error::DeregisterClient);
from_err!(ShutdownClientError, Error::ShutdownClient);
from_err!(ShutdownServerError, Error::ShutdownServer);
from_err!(RpcError, Error::Rpc);

fn discard_inner<A>(e: NotifyError<A>) -> NotifyError<()> {
    match e {
        NotifyError::Io(e) => NotifyError::Io(e),
        NotifyError::Full(..) => NotifyError::Full(()),
        NotifyError::Closed(Some(..)) => NotifyError::Closed(None),
        NotifyError::Closed(None) => NotifyError::Closed(None),
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

/// Provides the tarpc client and server, which implements the tarpc protocol.
/// The protocol is defined by the implementation.
pub mod protocol;

/// Provides the macro used for constructing rpc services and client stubs.
pub mod macros;
