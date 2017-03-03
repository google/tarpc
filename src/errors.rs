// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use serde::{Deserialize, Serialize};
use std::{fmt, io};
use std::error::Error as StdError;

/// All errors that can occur during the use of tarpc.
#[derive(Debug)]
pub enum Error<E> {
    /// Any IO error.
    Io(io::Error),
    /// Error deserializing the server response.
    ///
    /// Typically this indicates a faulty implementation of `serde::Serialize` or
    /// `serde::Deserialize`.
    ResponseDeserialize(::bincode::Error),
    /// Error deserializing the client request.
    ///
    /// Typically this indicates a faulty implementation of `serde::Serialize` or
    /// `serde::Deserialize`.
    RequestDeserialize(String),
    /// The server was unable to reply to the rpc for some reason.
    ///
    /// This is a service-specific error. Its type is individually specified in the
    /// `service!` macro for each rpc.
    App(E),
}

impl<E: StdError + Deserialize + Serialize + Send + 'static> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::ResponseDeserialize(ref e) => write!(f, r#"{}: "{}""#, self.description(), e),
            Error::RequestDeserialize(ref e) => write!(f, r#"{}: "{}""#, self.description(), e),
            Error::App(ref e) => fmt::Display::fmt(e, f),
            Error::Io(ref e) => fmt::Display::fmt(e, f),
        }
    }
}

impl<E: StdError + Deserialize + Serialize + Send + 'static> StdError for Error<E> {
    fn description(&self) -> &str {
        match *self {
            Error::ResponseDeserialize(_) => "The client failed to deserialize the response.",
            Error::RequestDeserialize(_) => "The server failed to deserialize the request.",
            Error::App(ref e) => e.description(),
            Error::Io(ref e) => e.description(),
        }
    }

    fn cause(&self) -> Option<&StdError> {
        match *self {
            Error::ResponseDeserialize(ref e) => e.cause(),
            Error::RequestDeserialize(_) |
            Error::App(_) => None,
            Error::Io(ref e) => e.cause(),
        }
    }
}

impl<E> From<io::Error> for Error<E> {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}

impl<E> From<WireError<E>> for Error<E> {
    fn from(err: WireError<E>) -> Self {
        match err {
            WireError::RequestDeserialize(s) => Error::RequestDeserialize(s),
            WireError::App(e) => Error::App(e),
        }
    }
}

/// A serializable, server-supplied error.
#[doc(hidden)]
#[derive(Deserialize, Serialize, Clone, Debug)]
pub enum WireError<E> {
    /// Error in serializing the server response or deserializing the client request.
    RequestDeserialize(String),
    /// The server was unable to reply to the rpc for some reason.
    App(E),
}

/// Convert `native_tls::Error` to `std::io::Error`
#[cfg(feature = "tls")]
pub fn native_to_io(e: ::native_tls::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}
