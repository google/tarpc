// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use std::error::Error as StdError;
use std::{fmt, io};

/// All errors that can occur during the use of tarpc.
#[derive(Debug)]
pub enum Error {
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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::ResponseDeserialize(ref e) => write!(f, r#"{}: "{}""#, self.description(), e),
            Error::RequestDeserialize(ref e) => write!(f, r#"{}: "{}""#, self.description(), e),
            Error::Io(ref e) => fmt::Display::fmt(e, f),
        }
    }
}

impl StdError for Error {
    fn description(&self) -> &str {
        match *self {
            Error::ResponseDeserialize(_) => "The client failed to deserialize the response.",
            Error::RequestDeserialize(_) => "The server failed to deserialize the request.",
            Error::Io(ref e) => e.description(),
        }
    }

    fn cause(&self) -> Option<&StdError> {
        match *self {
            Error::ResponseDeserialize(ref e) => e.cause(),
            Error::RequestDeserialize(_) => None,
            Error::Io(ref e) => e.cause(),
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}

impl From<WireError> for Error {
    fn from(err: WireError) -> Self {
        match err {
            WireError::RequestDeserialize(s) => Error::RequestDeserialize(s),
        }
    }
}

/// A serializable, server-supplied error.
#[doc(hidden)]
#[derive(Deserialize, Serialize, Clone, Debug)]
pub enum WireError {
    /// Server-side error in deserializing the client request.
    RequestDeserialize(String),
}

/// Convert `native_tls::Error` to `std::io::Error`
#[cfg(feature = "tls")]
pub fn native_to_io(e: ::native_tls::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}
