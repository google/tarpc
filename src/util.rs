// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use futures::stream::Stream;
use futures::{Future, IntoFuture, Poll};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use std::{fmt, io, mem};

/// A bottom type that impls `Error`, `Serialize`, and `Deserialize`. It is impossible to
/// instantiate this type.
#[allow(unreachable_code)]
pub struct Never(!);

impl fmt::Debug for Never {
    fn fmt(&self, _: &mut fmt::Formatter) -> fmt::Result {
        self.0
    }
}

impl Error for Never {
    fn description(&self) -> &str {
        self.0
    }
}

impl fmt::Display for Never {
    fn fmt(&self, _: &mut fmt::Formatter) -> fmt::Result {
        self.0
    }
}

impl Future for Never {
    type Item = Never;
    type Error = Never;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.0
    }
}

impl Stream for Never {
    type Item = Never;
    type Error = Never;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.0
    }
}

impl Serialize for Never {
    fn serialize<S>(&self, _: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0
    }
}

// Please don't try to deserialize this. :(
impl<'a> Deserialize<'a> for Never {
    fn deserialize<D>(_: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'a>,
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

/// Creates a new future which will eventually be the same as the one created
/// by calling the closure provided with the arguments provided.
///
/// The provided closure is only run once the future has a callback scheduled
/// on it, otherwise the callback never runs. Once run, however, this future is
/// the same as the one the closure creates.
pub fn lazy<F, A, R>(f: F, args: A) -> Lazy<F, A, R>
where
    F: FnOnce(A) -> R,
    R: IntoFuture,
{
    Lazy {
        inner: _Lazy::First(f, args),
    }
}

/// A future which defers creation of the actual future until a callback is
/// scheduled.
///
/// This is created by the `lazy` function.
#[derive(Debug)]
#[must_use = "futures do nothing unless polled"]
pub struct Lazy<F, A, R: IntoFuture> {
    inner: _Lazy<F, A, R::Future>,
}

#[derive(Debug)]
enum _Lazy<F, A, R> {
    First(F, A),
    Second(R),
    Moved,
}

impl<F, A, R> Lazy<F, A, R>
where
    F: FnOnce(A) -> R,
    R: IntoFuture,
{
    fn get(&mut self) -> &mut R::Future {
        match self.inner {
            _Lazy::First(..) => {}
            _Lazy::Second(ref mut f) => return f,
            _Lazy::Moved => panic!(), // can only happen if `f()` panics
        }
        match mem::replace(&mut self.inner, _Lazy::Moved) {
            _Lazy::First(f, args) => self.inner = _Lazy::Second(f(args).into_future()),
            _ => panic!(), // we already found First
        }
        match self.inner {
            _Lazy::Second(ref mut f) => f,
            _ => panic!(), // we just stored Second
        }
    }
}

impl<F, A, R> Future for Lazy<F, A, R>
where
    F: FnOnce(A) -> R,
    R: IntoFuture,
{
    type Item = R::Item;
    type Error = R::Error;

    fn poll(&mut self) -> Poll<R::Item, R::Error> {
        self.get().poll()
    }
}
