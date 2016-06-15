// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

//! An RPC library for Rust.
//!
//! Example usage:
//!
//! ```
//! #![feature(default_type_parameter_fallback, try_from)]
//! #[macro_use]
//! extern crate tarpc;
//!
//! use tarpc::{Client, RpcResult};
//!
//! service! {
//!     rpc hello(name: String) -> String;
//!     rpc add(x: i32, y: i32) -> i32;
//! }
//!
//! #[derive(Clone, Copy)]
//! struct Server;
//!
//! impl SyncService for Server {
//!     fn hello(&self, s: String) -> RpcResult<String> {
//!         Ok(format!("Hello, {}!", s))
//!     }
//!
//!     fn add(&self, x: i32, y: i32) -> RpcResult<i32> {
//!         Ok(x + y)
//!     }
//! }
//!
//! fn main() {
//!     let serve_handle = Server.listen("localhost:0").unwrap();
//!     let client = SyncClient::connect(serve_handle.local_addr()).unwrap();
//!     assert_eq!(3, client.add(&1, &2).unwrap());
//!     assert_eq!("Hello, Mom!", client.hello(&"Mom".to_string()).unwrap());
//! }
//! ```
//!
#![deny(missing_docs)]
#![feature(custom_derive, plugin, default_type_parameter_fallback, iter_arith, try_from)]
#![plugin(serde_macros)]

extern crate bincode;
extern crate byteorder;
extern crate bytes;
extern crate fnv;
#[macro_use]
extern crate lazy_static;
extern crate num_cpus;
#[macro_use]
extern crate quick_error;
extern crate rand;
extern crate threadpool;

/// Re-exported for use by macros.
pub extern crate serde;
/// Re-exported for use by macros.
pub extern crate mio;
/// Re-exported for use by macros.
#[macro_use]
pub extern crate log;

use mio::NotifyError;
use std::error;
use std::fmt;
use std::io;
use std::sync::mpsc;

macro_rules! pos {
    () => (concat!(file!(), ":", line!()))
}

macro_rules! id_wrapper {
    ($name: ident) => {
        /// A thin wrapper around u64 to disambiguate from other types of id's.
        #[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
        pub struct $name(u64);

        impl ::std::ops::AddAssign<u64> for $name {
            fn add_assign(&mut self, amount: u64) {
                self.0 += amount;
            }
        }

        impl ::std::ops::SubAssign<u64> for $name {
            fn sub_assign(&mut self, amount: u64) {
                self.0 -= amount;
            }
        }
    };
}

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
        /// The service is experiencing high traffic volume; retry the request after a backoff
        /// period.
        Busy {}
        /// The client connected to a tarpc service that did not recognize the client request.
        WrongService(desc: String) {
            description(&desc)
        }
        /// Any IO error other than ConnectionBroken.
        Io(err: io::Error) {
            cause(err)
            from()
            description(err.description())
        }
        /// Error in receiving a value, typically from an event loop that may have shutdown.
        Rx(err: mpsc::RecvError) {
            cause(err)
            from()
            description(err.description())
        }
        /// Error in deserializing, either on client or server.
        Deserialize(err: bincode::serde::DeserializeError) {
            cause(err)
            from()
            description(err.description())
        }
        /// Error in serializing, either on client or server.
        Serialize(err: bincode::serde::SerializeError) {
            cause(err)
            from()
            description(err.description())
        }
        /// Error in sending a notification to the client event loop.
        ClientNotify(err: NotifyError<protocol::client::Action>) {
            cause(err)
            from()
            description(err.description())
        }
        /// Error in sending a notification to the server event loop.
        ServerNotify(err: NotifyError<protocol::server::Action>) {
            cause(err)
            from()
            description(err.description())
        }
        /// The server was unable to reply to the rpc for some reason.
        Rpc(err: RpcError) {
            cause(err)
            from()
            description(err.description())
        }
    }
}

impl From<CanonicalRpcError> for Error {
    fn from(err: CanonicalRpcError) -> Self {
        info!("Err: {:?}", err);
        match err.code {
            CanonicalRpcErrorCode::Service(code) => {
                Error::Rpc(RpcError {
                    code: code,
                    description: err.description,
                })
            }
            CanonicalRpcErrorCode::WrongService => Error::WrongService(err.description),
            CanonicalRpcErrorCode::Busy => Error::Busy,
        }
    }
}

/// A serializable, server-supplied error.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct RpcError {
    /// The type of error that occurred.
    pub code: RpcErrorCode,
    /// More details about the error.
    pub description: String,
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.description)
    }
}

impl error::Error for RpcError {
    fn description(&self) -> &str {
        &self.description
    }
}

/// A serializable, server-supplied error that is a superset of `RpcError`. In addition to
/// transmitting errors that the service writer may concern themselves with, it also transmits
/// some errors automatically. For example, if request deserialization fails, the framework
/// will automatically respond with a `WrongService` error, which all tarpc clients will
/// recognize.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct CanonicalRpcError {
    /// The type of error that occurred.
    pub code: CanonicalRpcErrorCode,
    /// More details about the error.
    pub description: String,
}

impl fmt::Display for CanonicalRpcError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.description)
    }
}

impl error::Error for CanonicalRpcError {
    fn description(&self) -> &str {
        &self.description
    }
}

/// Combines error codes propagated automatically by the tarpc framework, as well as error codes
/// propagated by the service. Is a superset of `RpcErrorCode`.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub enum CanonicalRpcErrorCode {
    /// The service returned an error.
    Service(RpcErrorCode),
    /// The service could not interpret the request.
    WrongService,
    /// The service is experiencing high traffic volume; retry the request after a backoff period.
    Busy,
}

impl fmt::Display for CanonicalRpcErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            CanonicalRpcErrorCode::Service(ref code) => write!(f, "{}", code),
            CanonicalRpcErrorCode::WrongService => write!(f, "Wrong service"),
            CanonicalRpcErrorCode::Busy => {
                write!(f, "The server was too busy to serve the request")
            }
        }
    }
}

impl From<Error> for CanonicalRpcError {
    fn from(err: Error) -> Self {
        match err {
            Error::Busy => {
                CanonicalRpcError {
                    code: CanonicalRpcErrorCode::Busy,
                    description: "".to_string(),
                }
            }
            Error::WrongService(desc) => {
                CanonicalRpcError {
                    code: CanonicalRpcErrorCode::WrongService,
                    description: desc,
                }
            }
            Error::Rpc(e) => {
                CanonicalRpcError {
                    code: CanonicalRpcErrorCode::Service(e.code),
                    description: e.description,
                }
            }
            e => {
                CanonicalRpcError {
                    code: CanonicalRpcErrorCode::Service(RpcErrorCode::Internal),
                    description: error::Error::description(&e).to_string(),
                }
            }
        }
    }
}

impl From<RpcError> for CanonicalRpcError {
    fn from(err: RpcError) -> Self {
        CanonicalRpcError {
            description: err.description,
            code: CanonicalRpcErrorCode::Service(err.code),
        }
    }
}

/// Reasons an rpc failed.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum RpcErrorCode {
    /// An internal error occurred on the server.
    Internal,
    /// The user input failed a precondition of the rpc method.
    BadRequest,
}

impl fmt::Display for RpcErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            RpcErrorCode::Internal => write!(f, "Internal error"),
            RpcErrorCode::BadRequest => write!(f, "Bad request"),
        }
    }
}

impl From<Error> for RpcError {
    fn from(err: Error) -> Self {
        match err {
            Error::Rpc(e) => e,
            e => {
                RpcError {
                    description: error::Error::description(&e).to_string(),
                    code: RpcErrorCode::Internal,
                }
            }
        }
    }
}

/// Return type of rpc calls: either the successful return value, or a client error.
pub type Result<T> = ::std::result::Result<T, Error>;
/// Return type from server to client. Converted into ```Result<T>``` before reaching the user.
pub type RpcResult<T> = ::std::result::Result<T, CanonicalRpcError>;

pub use cached_pool::CachedPool;
pub use protocol::RpcId;
pub use protocol::server::{self, Ctx, ServeHandle};
pub use protocol::client::{self, Client, Future};
pub use transport::{Listener, Stream};

/// Provides the tarpc client and server, which implements the tarpc protocol.
/// The protocol is defined by the implementation.
pub mod protocol;

/// Provides an automatically-growing thread pool whose threads expire
/// after a configurable amount of time.
pub mod cached_pool;

/// Provides the macro used for constructing rpc services and client stubs.
mod macros;

/// Provides the `Stream` and `Listener` types.
mod transport;
