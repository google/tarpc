// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use {WireError, framed};
use bincode::serde::DeserializeError;
use futures::{self, Future};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;
use tokio_core::net::TcpStream;
use tokio_proto::BindClient as ProtoBindClient;
use tokio_proto::multiplex::Multiplex;
use tokio_service::Service;

type WireResponse<Resp, E> = Result<Result<Resp, WireError<E>>, DeserializeError>;
type ResponseFuture<Req, Resp, E> = futures::Map<<BindClient<Req, Resp, E> as Service>::Future,
                                            fn(WireResponse<Resp, E>) -> Result<Resp, ::Error<E>>>;
type BindClient<Req, Resp, E> =
    <framed::Proto<Req, Result<Resp, WireError<E>>> as ProtoBindClient<Multiplex, TcpStream>>::BindClient;

/// A client that impls `tokio_service::Service` that writes and reads bytes.
///
/// Typically, this would be combined with a serialization pre-processing step
/// and a deserialization post-processing step.
pub struct Client<Req, Resp, E>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static,
{
    inner: BindClient<Req, Resp, E>,
}

impl<Req, Resp, E> Service for Client<Req, Resp, E>
    where Req: Serialize + Sync + Send + 'static,
          Resp: Deserialize + Sync + Send + 'static,
          E: Deserialize + Sync + Send + 'static
{
    type Request = Req;
    type Response = Result<Resp, ::Error<E>>;
    type Error = io::Error;
    type Future = ResponseFuture<Req, Resp, E>;

    fn call(&self, request: Self::Request) -> Self::Future {
        self.inner.call(request).map(Self::map_err)
    }
}

impl<Req, Resp, E> Client<Req, Resp, E>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static,
{
    fn new(inner: BindClient<Req, Resp, E>) -> Self
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static
    {
        Client {
            inner: inner,
        }
    }

    fn map_err(resp: WireResponse<Resp, E>) -> Result<Resp, ::Error<E>> {
        resp.map(|r| r.map_err(::Error::from))
            .map_err(::Error::ClientDeserialize)
            .and_then(|r| r)
    }
}

impl<Req, Resp, E> fmt::Debug for Client<Req, Resp, E>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "Client {{ .. }}")
    }
}

/// Exposes a trait for connecting asynchronously to servers.
pub mod future {
    use {REMOTE, framed};
    use futures::{self, Async, Future};
    use serde::{Deserialize, Serialize};
    use std::io;
    use std::marker::PhantomData;
    use std::net::SocketAddr;
    use super::Client;
    use tokio_core::net::TcpStream;
    use tokio_core::{self, reactor};
    use tokio_proto::BindClient;

    /// Types that can connect to a server asynchronously.
    pub trait Connect<'a>: Sized {
        /// The type of the future returned when calling `connect`.
        type ConnectFut: Future<Item = Self, Error = io::Error> + 'static;

        /// The type of the future returned when calling `connect_with`.
        type ConnectWithFut: Future<Item = Self, Error = io::Error> + 'a;

        /// Connects to a server located at the given address, using a remote to the default
        /// reactor.
        fn connect(addr: &SocketAddr) -> Self::ConnectFut {
            Self::connect_remotely(addr, &REMOTE)
        }

        /// Connects to a server located at the given address, using the given reactor remote.
        fn connect_remotely(addr: &SocketAddr, remote: &reactor::Remote) -> Self::ConnectFut;

        /// Connects to a server located at the given address, using the given reactor handle.
        fn connect_with(addr: &SocketAddr, handle: &'a reactor::Handle) -> Self::ConnectWithFut;
    }

    /// A future that resolves to a `Client` or an `io::Error`.
    pub struct ConnectFuture<Req, Resp, E>
        where Req: Serialize + 'static,
              Resp: Deserialize + 'static,
              E: Deserialize + 'static,
    {
        inner: futures::Oneshot<io::Result<Client<Req, Resp, E>>>,
    }

    impl<Req, Resp, E> Future for ConnectFuture<Req, Resp, E>
        where Req: Serialize + 'static,
              Resp: Deserialize + 'static,
              E: Deserialize + 'static,
    {
        type Item = Client<Req, Resp, E>;
        type Error = io::Error;

        fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
            // Ok to unwrap because we ensure the oneshot is always completed.
            match self.inner.poll().unwrap() {
                Async::Ready(Ok(client)) => Ok(Async::Ready(client)),
                Async::Ready(Err(err)) => Err(err),
                Async::NotReady => Ok(Async::NotReady),
            }
        }
    }

    /// A future that resolves to a `Client` or an `io::Error`.
    pub struct ConnectWithFuture<'a, Req, Resp, E> {
        inner: futures::Map<tokio_core::net::TcpStreamNew,
                            MultiplexConnect<'a, Req, Resp, E>>,
    }

    impl<'a, Req, Resp, E> Future for ConnectWithFuture<'a, Req, Resp, E>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static
    {
        type Item = Client<Req, Resp, E>;
        type Error = io::Error;

        fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
            self.inner.poll()
        }
    }

    struct MultiplexConnect<'a, Req, Resp, E>(&'a reactor::Handle, PhantomData<(Req, Resp, E)>);

    impl<'a, Req, Resp, E> MultiplexConnect<'a, Req, Resp, E> {
        fn new(handle: &'a reactor::Handle) -> Self {
            MultiplexConnect(handle, PhantomData)
        }
    }

    impl<'a, Req, Resp, E> FnOnce<(TcpStream,)> for MultiplexConnect<'a, Req, Resp, E>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static
    {
        type Output = Client<Req, Resp, E>;

        extern "rust-call" fn call_once(self, (tcp,): (TcpStream,)) -> Client<Req, Resp, E> {
            Client::new(framed::Proto::new().bind_client(self.0, tcp))
        }
    }

    impl<'a, Req, Resp, E> Connect<'a> for Client<Req, Resp, E>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static
    {
        type ConnectFut = ConnectFuture<Req, Resp, E>;
        type ConnectWithFut = ConnectWithFuture<'a, Req, Resp, E>;

        fn connect_remotely(addr: &SocketAddr, remote: &reactor::Remote) -> Self::ConnectFut {
            let addr = *addr;
            let (tx, rx) = futures::oneshot();
            remote.spawn(move |handle| {
                let handle2 = handle.clone();
                TcpStream::connect(&addr, handle)
                    .map(move |tcp| Client::new(framed::Proto::new().bind_client(&handle2, tcp)))
                    .then(move |result| {
                        tx.complete(result);
                        Ok(())
                    })
            });
            ConnectFuture { inner: rx }
        }

        fn connect_with(addr: &SocketAddr, handle: &'a reactor::Handle) -> Self::ConnectWithFut {
            ConnectWithFuture {
                inner: TcpStream::connect(addr, handle).map(MultiplexConnect::new(handle))
            }
        }
    }
}

/// Exposes a trait for connecting synchronously to servers.
pub mod sync {
    use futures::Future;
    use serde::{Deserialize, Serialize};
    use std::io;
    use std::net::ToSocketAddrs;
    use super::Client;

    /// Types that can connect to a server synchronously.
    pub trait Connect: Sized {
        /// Connects to a server located at the given address.
        fn connect<A>(addr: A) -> Result<Self, io::Error> where A: ToSocketAddrs;
    }

    impl<Req, Resp, E> Connect for Client<Req, Resp, E>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static
    {
        fn connect<A>(addr: A) -> Result<Self, io::Error>
            where A: ToSocketAddrs
        {
            let addr = if let Some(a) = addr.to_socket_addrs()?.next() {
                a
            } else {
                return Err(io::Error::new(io::ErrorKind::AddrNotAvailable,
                                          "`ToSocketAddrs::to_socket_addrs` returned an empty \
                                           iterator."));
            };
            <Self as super::future::Connect>::connect(&addr).wait()
        }
    }
}
