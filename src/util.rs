// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use futures;
use std::fmt;
use std::error::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A trait for easy spawning of futures.
///
/// `Future`s don't actually perform their computations until spawned via an `Executor`. Typically,
/// an executor will be associated with an event loop. The `fn` provided by `Spawn` handles
/// spawning the future on the static event loop on which tarpc clients and servers run by default.
pub trait Spawn {
    /// Spawns a future on the default event loop.
    fn spawn(self);
}

impl<F> Spawn for F
    where F: futures::Future + Send + 'static
{
    fn spawn(self) {
        ::protocol::LOOP_HANDLE.spawn(move |_| self.then(|_| Ok::<(), ()>(())))
    }
}

/// A bottom type that impls `Error`, `Serialize`, and `Deserialize`. It is impossible to
/// instantiate this type.
#[derive(Debug)]
pub struct Never(!);

impl Error for Never {
    fn description(&self) -> &str {
        unreachable!()
    }
}

impl fmt::Display for Never {
    fn fmt(&self, _: &mut fmt::Formatter) -> fmt::Result {
        unreachable!()
    }
}

impl Serialize for Never {
    fn serialize<S>(&self, _: &mut S) -> Result<(), S::Error>
        where S: Serializer
    {
        unreachable!()
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
