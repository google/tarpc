// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode;
use serde::{Deserialize, Serialize};
use std::{fmt, io};
use std::error::Error as StdError;

/// All errors that can occur during the use of tarpc.
#[derive(Debug)]
pub enum Error<E> {
    /// Any IO error.
    Io(io::Error),
    /// Error in deserializing a server response.
    ///
    /// Typically this indicates a faulty implementation of `serde::Serialize` or
    /// `serde::Deserialize`.
    ClientDeserialize(bincode::serde::DeserializeError),
    /// Error in serializing a client request.
    ///
    /// Typically this indicates a faulty implementation of `serde::Serialize`.
    ClientSerialize(bincode::serde::SerializeError),
    /// Error in deserializing a client request.
    ///
    /// Typically this indicates a faulty implementation of `serde::Serialize` or
    /// `serde::Deserialize`.
    ServerDeserialize(String),
    /// Error in serializing a server response.
    ///
    /// Typically this indicates a faulty implementation of `serde::Serialize`.
    ServerSerialize(String),
    /// The server was unable to reply to the rpc for some reason.
    ///
    /// This is a service-specific error. Its type is individually specified in the
    /// `service!` macro for each rpc.
    App(E),
}

impl<E: SerializableError> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::ClientDeserialize(ref e) => write!(f, r#"{}: "{}""#, self.description(), e),
            Error::ClientSerialize(ref e) => write!(f, r#"{}: "{}""#, self.description(), e),
            Error::ServerDeserialize(ref e) => write!(f, r#"{}: "{}""#, self.description(), e),
            Error::ServerSerialize(ref e) => write!(f, r#"{}: "{}""#, self.description(), e),
            Error::App(ref e) => fmt::Display::fmt(e, f),
            Error::Io(ref e) => fmt::Display::fmt(e, f),
        }
    }
}

impl<E: SerializableError> StdError for Error<E> {
    fn description(&self) -> &str {
        match *self {
            Error::ClientDeserialize(_) => "The client failed to deserialize the server response.",
            Error::ClientSerialize(_) => "The client failed to serialize the request.",
            Error::ServerDeserialize(_) => "The server failed to deserialize the request.",
            Error::ServerSerialize(_) => "The server failed to serialize the response.",
            Error::App(ref e) => e.description(),
            Error::Io(ref e) => e.description(),
        }
    }

    fn cause(&self) -> Option<&StdError> {
        match *self {
            Error::ClientDeserialize(ref e) => e.cause(),
            Error::ClientSerialize(ref e) => e.cause(),
            Error::ServerDeserialize(_) |
            Error::ServerSerialize(_) |
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
            WireError::ServerDeserialize(s) => Error::ServerDeserialize(s),
            WireError::ServerSerialize(s) => Error::ServerSerialize(s),
            WireError::App(e) => Error::App(e),
        }
    }
}

/// A serializable, server-supplied error.
#[doc(hidden)]
#[derive(Deserialize, Serialize, Clone, Debug)]
pub enum WireError<E> {
    /// Error in deserializing a client request.
    ServerDeserialize(String),
    /// Error in serializing server response.
    ServerSerialize(String),
    /// The server was unable to reply to the rpc for some reason.
    App(E),
}

/// A serializable error.
pub trait SerializableError: StdError + Deserialize + Serialize + Send + 'static {}

impl<E: StdError + Deserialize + Serialize + Send + 'static> SerializableError for E {}

#[cfg(feature = "tls")]
/// Convert `native_tls::Error` to `std::io::Error`
pub fn native2io(e: ::native_tls::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}
