// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use std::{fmt, io};

/// A `String` that impls `std::error::Error`. Useful for quick-and-dirty error propagation.
#[derive(Debug, Serialize, Deserialize)]
pub struct Message(pub String);

impl Error for Message {
    fn description(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl<S: Into<String>> From<S> for Message {
    fn from(s: S) -> Self {
        Message(s.into())
    }
}

/// Provides a utility method for more ergonomically parsing a `SocketAddr` when only one is
/// needed.
pub trait FirstSocketAddr: ToSocketAddrs {
    /// Returns the first resolved `SocketAddr`, if one exists.
    fn try_first_socket_addr(&self) -> io::Result<SocketAddr> {
        if let Some(a) = self.to_socket_addrs()?.next() {
            Ok(a)
        } else {
            Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "`ToSocketAddrs::to_socket_addrs` returned an empty iterator.",
            ))
        }
    }

    /// Returns the first resolved `SocketAddr` or panics otherwise.
    fn first_socket_addr(&self) -> SocketAddr {
        self.try_first_socket_addr().unwrap()
    }
}

impl<A: ToSocketAddrs> FirstSocketAddr for A {}
