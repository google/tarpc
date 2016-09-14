// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use Packet;
use futures::{Async, BoxFuture};
use futures::stream::Empty;
use std::fmt;
use std::io;
use tokio_proto::pipeline;
use tokio_service::Service;
use util::Never;

/// A client `Service` that writes and reads bytes.
///
/// Typically, this would be combined with a serialization pre-processing step
/// and a deserialization post-processing step.
#[derive(Clone)]
pub struct Client {
    inner: pipeline::Client<Packet, Vec<u8>, Empty<Never, io::Error>, io::Error>,
}

impl Service for Client {
    type Request = Packet;
    type Response = Vec<u8>;
    type Error = io::Error;
    type Future = BoxFuture<Vec<u8>, io::Error>;

    fn poll_ready(&self) -> Async<()> {
        Async::Ready(())
    }

    fn call(&self, request: Packet) -> Self::Future {
        self.inner.call(pipeline::Message::WithoutBody(request))
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "Client {{ .. }}")
    }
}

/// Exposes a trait for connecting asynchronously to servers.
pub mod future {
    use futures::{self, Async, Future};
    use protocol::{LOOP_HANDLE, TarpcTransport};
    use std::cell::RefCell;
    use std::io;
    use std::net::SocketAddr;
    use super::Client;
    use tokio_core::net::TcpStream;
    use tokio_proto::pipeline;


    /// Types that can connect to a server asynchronously.
    pub trait Connect: Sized {
        /// The type of the future returned when calling connect.
        type Fut: Future<Item = Self, Error = io::Error>;

        /// Connects to a server located at the given address.
        fn connect(addr: &SocketAddr) -> Self::Fut;
    }

    /// A future that resolves to a `Client` or an `io::Error`.
    pub struct ClientFuture {
        inner: futures::Oneshot<io::Result<Client>>,
    }

    impl Future for ClientFuture {
        type Item = Client;
        type Error = io::Error;

        fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
            match self.inner.poll().unwrap() {
                Async::Ready(Ok(client)) => Ok(Async::Ready(client)),
                Async::Ready(Err(err)) => Err(err),
                Async::NotReady => Ok(Async::NotReady),
            }
        }
    }

    impl Connect for Client {
        type Fut = ClientFuture;

        /// Starts an event loop on a thread and registers a new client
        /// connected to the given address.
        fn connect(addr: &SocketAddr) -> ClientFuture {
            let addr = *addr;
            let (tx, rx) = futures::oneshot();
            LOOP_HANDLE.spawn(move |handle| {
                let handle2 = handle.clone();
                TcpStream::connect(&addr, handle)
                    .and_then(move |tcp| {
                        let tcp = RefCell::new(Some(tcp));
                        let c = try!(pipeline::connect(&handle2, move || {
                            Ok(TarpcTransport::new(tcp.borrow_mut().take().unwrap()))
                        }));
                        Ok(Client { inner: c })
                    })
                    .then(|client| Ok(tx.complete(client)))
            });
            ClientFuture { inner: rx }
        }
    }
}

/// Exposes a trait for connecting synchronously to servers.
pub mod sync {
    use futures::Future;
    use std::io;
    use std::net::ToSocketAddrs;
    use super::Client;

    /// Types that can connect to a server synchronously.
    pub trait Connect: Sized {
        /// Connects to a server located at the given address.
        fn connect<A>(addr: A) -> Result<Self, io::Error> where A: ToSocketAddrs;
    }

    impl Connect for Client {
        fn connect<A>(addr: A) -> Result<Self, io::Error>
            where A: ToSocketAddrs
        {
            let addr = if let Some(a) = try!(addr.to_socket_addrs()).next() {
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
