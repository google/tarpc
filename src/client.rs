// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use {RpcError, serde};
use futures::{self, Future, Poll, Task};
use protocol::{REACTOR, TarpcTransport, deserialize};
use protocol::writer::{self, Packet};
use std::fmt;
use std::net::ToSocketAddrs;
use tokio::Service;
use tokio::proto::pipeline;
use tokio::util::future::Val;

/// Types that can connect to a server.
pub trait Connect: Sized {
    /// Connects to a server located at the given address.
    fn connect<A>(addr: A) -> ::Result<Self> where A: ToSocketAddrs;
}

/// A low-level client for communicating with a service. Reads and writes byte buffers. Typically
/// a type-aware client will be built on top of this.
#[derive(Clone, Copy, Debug)]
pub struct Client;

impl Client {
    /// Starts an event loop on a thread and registers a new client connected to the given address.
    pub fn connect<A: ToSocketAddrs>(self, addr: A) -> ::Result<Handle> {
        let mut addrs = try!(addr.to_socket_addrs());
        let addr = if let Some(a) = addrs.next() {
            a
        } else {
            return Err(::Error::NoAddressFound);
        };
        let client = pipeline::connect(&REACTOR.lock().unwrap(),
                                       addr,
                                       |stream| Ok(TarpcTransport::new(stream)));
        Ok(Handle { inner: client })
    }
}

/// A thin wrapper around `pipeline::ClientHandle` that handles Serialization.
#[derive(Clone)]
pub struct Handle {
    inner: pipeline::ClientHandle<writer::Packet, Vec<u8>, ::Error>,
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "Handle {{ .. }}")
    }
}

impl Handle {
    /// Send a request to the server. Returns a future which can be blocked on until a reply is
    /// available.
    ///
    /// This method is generic over the request and the reply type, but typically a single client
    /// will only intend to send one type of request and receive one type of reply. This isn't
    /// encoded as type parameters in `Handle` because doing so would make it harder to
    /// run multiple different clients on the same event loop.
    #[inline]
    pub fn rpc<Req, Rep>(&self, req: &Req) -> Reply<Rep>
        where Req: serde::Serialize,
              Rep: serde::Deserialize + Send + 'static
    {
        let req = match Packet::new(&req) {
            Err(e) => return Reply(Fut::Failed(futures::failed(e))),
            Ok(req) => req,
        };
        return Reply(Fut::Called(self.inner.call(req).then(deserialize_message)));

        fn deserialize_message<Rep>(message: Result<Vec<u8>, ::Error>) -> Result<Rep, ::Error>
            where Rep: serde::Deserialize + Send + 'static
        {
            deserialize::<Result<_, RpcError>>(&message?)?.map_err(Into::into)
        }
    }
}

/// An rpc future.
pub struct Reply<T>(Fut<T>) where T: Send + 'static;

/// Type alias for a future chain.
enum Fut<T>
    where T: Send + 'static
{
    Called(futures::Then<Response, ::Result<T>, DeserializeFn<T>>),
    Failed(futures::Failed<T, ::Error>),
}

type Response = Val<Vec<u8>, ::Error>;
type DeserializeFn<T> = fn(::Result<Vec<u8>>) -> ::Result<T>;

impl<T> Future for Reply<T>
    where T: Send + 'static
{
    type Item = T;
    type Error = ::Error;

    fn poll(&mut self, task: &mut Task) -> Poll<T, ::Error> {
        match self.0 {
            Fut::Called(ref mut f) => f.poll(task),
            Fut::Failed(ref mut f) => f.poll(task),
        }
    }

    fn schedule(&mut self, task: &mut Task) {
        match self.0 {
            Fut::Called(ref mut f) => f.schedule(task),
            Fut::Failed(ref mut f) => f.schedule(task),
        }
    }
}
