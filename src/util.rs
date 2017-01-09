// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use futures::{self, Future, Poll};
use futures::stream::Stream;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::error::Error;
use std::{fmt, io, thread};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc;
use tokio_core::reactor;

/// A bottom type that impls `Error`, `Serialize`, and `Deserialize`. It is impossible to
/// instantiate this type.
#[allow(unreachable_code)]
pub struct Never(!);

impl fmt::Debug for Never {
    fn fmt(&self, _: &mut fmt::Formatter) -> fmt::Result {
        unreachable!("Never cannot be instantiated");
    }
}

impl Error for Never {
    fn description(&self) -> &str {
        match self.0 {
        }
    }
}

impl fmt::Display for Never {
    fn fmt(&self, _: &mut fmt::Formatter) -> fmt::Result {
        match self.0 {
        }
    }
}

impl Future for Never {
    type Item = Never;
    type Error = Never;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.0 {
        }
    }
}

impl Stream for Never {
    type Item = Never;
    type Error = Never;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match self.0 {
        }
    }
}

impl Serialize for Never {
    fn serialize<S>(&self, _: &mut S) -> Result<(), S::Error>
        where S: Serializer
    {
        match self.0 {
        }
    }
}

// Please don't try to deserialize this. :(
impl Deserialize for Never {
    fn deserialize<D>(_: &mut D) -> Result<Self, D::Error>
        where D: Deserializer
    {
        panic!("Never cannot be instantiated!");
    }
}

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
             Err(io::Error::new(io::ErrorKind::AddrNotAvailable,
                                "`ToSocketAddrs::to_socket_addrs` returned an empty iterator."))
        }
    }

    /// Returns the first resolved `SocketAddr` or panics otherwise.
    fn first_socket_addr(&self) -> SocketAddr {
        self.try_first_socket_addr().unwrap()
    }
}

impl<A: ToSocketAddrs> FirstSocketAddr for A {}

/// Spawns a `reactor::Core` running forever on a new thread.
pub fn spawn_core() -> reactor::Remote {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut core = reactor::Core::new().unwrap();
        tx.send(core.handle().remote().clone()).unwrap();

        // Run forever
        core.run(futures::empty::<(), !>()).unwrap();
    });
    rx.recv().unwrap()
}

/// A struct that will format as the contained type if the type impls Debug.
pub struct Debugger<'a, T: 'a>(pub &'a T);

impl<'a, T: fmt::Debug> fmt::Debug for Debugger<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:?}", self.0)
    }
}

impl<'a, T> fmt::Debug for Debugger<'a, T> {
    default fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{{not debuggable}}")
    }
}
