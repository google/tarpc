// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use WireError;
use bincode::serde::DeserializeError;
use futures::{self, Async, Future};
use std::fmt;
use std::io;
use tokio_proto::easy::{EasyClient, EasyResponse};
use tokio_service::Service;

/// A client `Service` that writes and reads bytes.
///
/// Typically, this would be combined with a serialization pre-processing step
/// and a deserialization post-processing step.
pub struct Client<Req, Resp, E> {
    inner: EasyClient<Req, WireResponse<Resp, E>>,
}

type WireResponse<Resp, E> = Result<Result<Resp, WireError<E>>, DeserializeError>;
type ResponseFuture<Resp, E> = futures::Map<EasyResponse<WireResponse<Resp, E>>,
                                            fn(WireResponse<Resp, E>) -> Result<Resp, ::Error<E>>>;

impl<Req, Resp, E> Service for Client<Req, Resp, E>
    where Req: Send + 'static,
          Resp: Send + 'static,
          E: Send + 'static
{
    type Request = Req;
    type Response = Result<Resp, ::Error<E>>;
    type Error = io::Error;
    type Future = ResponseFuture<Resp, E>;

    fn poll_ready(&self) -> Async<()> {
        self.inner.poll_ready()
    }

    fn call(&self, request: Self::Request) -> Self::Future {
        self.inner.call(request).map(Self::map_err)
    }
}

impl<Req, Resp, E> Client<Req, Resp, E> {
    fn map_err(resp: WireResponse<Resp, E>) -> Result<Resp, ::Error<E>> {
        resp.map(|r| r.map_err(::Error::from))
            .map_err(::Error::ClientDeserialize)
            .and_then(|r| r)
    }
}

impl<Req, Resp, E> fmt::Debug for Client<Req, Resp, E> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "Client {{ .. }}")
    }
}

/// Exposes a trait for connecting asynchronously to servers.
pub mod future {
    use REMOTE;
    use framed::Framed;
    use futures::{self, Async, Future};
    use serde::{Deserialize, Serialize};
    use std::io;
    use std::net::SocketAddr;
    use super::Client;
    use tokio_core::net::TcpStream;
    use tokio_proto::easy::multiplex;


    /// Types that can connect to a server asynchronously.
    pub trait Connect: Sized {
        /// The type of the future returned when calling connect.
        type Fut: Future<Item = Self, Error = io::Error>;

        /// Connects to a server located at the given address.
        fn connect(addr: &SocketAddr) -> Self::Fut;
    }

    /// A future that resolves to a `Client` or an `io::Error`.
    pub struct ClientFuture<Req, Resp, E> {
        inner: futures::Oneshot<io::Result<Client<Req, Resp, E>>>,
    }

    impl<Req, Resp, E> Future for ClientFuture<Req, Resp, E> {
        type Item = Client<Req, Resp, E>;
        type Error = io::Error;

        fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
            match self.inner.poll().unwrap() {
                Async::Ready(Ok(client)) => Ok(Async::Ready(client)),
                Async::Ready(Err(err)) => Err(err),
                Async::NotReady => Ok(Async::NotReady),
            }
        }
    }

    impl<Req, Resp, E> Connect for Client<Req, Resp, E>
        where Req: Serialize + Send + 'static,
              Resp: Deserialize + Send + 'static,
              E: Deserialize + Send + 'static
    {
        type Fut = ClientFuture<Req, Resp, E>;

        /// Starts an event loop on a thread and registers a new client
        /// connected to the given address.
        fn connect(addr: &SocketAddr) -> ClientFuture<Req, Resp, E> {
            let addr = *addr;
            let (tx, rx) = futures::oneshot();
            REMOTE.spawn(move |handle| {
                let handle2 = handle.clone();
                TcpStream::connect(&addr, handle).then(move |tcp| {
                    match tcp {
                        Ok(tcp) => {
                            tx.complete(Ok(Client {
                                inner: multiplex::connect(Framed::new(tcp), &handle2),
                            }));
                        }
                        Err(e) => {
                            tx.complete(Err(e));
                        }
                    }
                    Ok(())
                })
            });
            ClientFuture { inner: rx }
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
        where Req: Serialize + Send + 'static,
              Resp: Deserialize + Send + 'static,
              E: Deserialize + Send + 'static
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
