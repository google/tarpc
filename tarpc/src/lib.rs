//! An RPC library for Rust.
//!
//! Example usage:
//!
//! ```
//! # #![feature(custom_derive)]
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
//!
//! impl my_server::Service for () {
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
//!     let shutdown = my_server::serve(addr, ()).unwrap();
//!     let client = Client::new(addr).unwrap();
//!     assert_eq!(3, client.add(1, 2).unwrap());
//!     assert_eq!("Hello, Mom!".to_string(),
//!                client.hello("Mom".to_string()).unwrap());
//!     drop(client);
//!     shutdown.shutdown();
//! }
//! ```

#![feature(trace_macros)]
#![feature(const_fn)]
#![feature(braced_empty_structs)]
#![feature(custom_derive, plugin)]
#![plugin(serde_macros)]
#![deny(missing_docs)]

extern crate serde;
extern crate bincode;
#[macro_use]
extern crate log;

use std::io;
use std::convert::From;

/// Provides the tarpc client and server, which implements the tarpc protocol.
/// The protocol is defined by the implementation.
pub mod protocol;

/// Provides the macro used for constructing rpc services and client stubs.
pub mod macros;

/// An error that occurred while processing an RPC request
#[derive(Debug)]
pub enum Error {
    #[doc="An IO error occurred."]
    Io(io::Error),

    #[doc="An unexpected internal error. Typically a bug in the server impl."]
    InternalError,
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(err)
    }
}

impl From<protocol::Error> for Error {
    fn from(err: protocol::Error) -> Error {
        match err {
            protocol::Error::Io(err) => Error::Io(err),
            _ => Error::InternalError,
        }
    }
}

///The result of an RPC call; either the successful result or the error
pub type Result<T> = ::std::result::Result<T, Error>;
