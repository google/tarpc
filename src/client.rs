// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use Packet;
use futures::BoxFuture;
use futures::stream::Empty;
use std::fmt;
use std::io;
use tokio_service::Service;
use tokio_proto::pipeline;

/// A client `Service` that writes and reads bytes.
///
/// Typically, this would be combined with a serialization pre-processing step
/// and a deserialization post-processing step.
#[derive(Clone)]
pub struct Client {
    inner: pipeline::Client<Packet, Vec<u8>, Empty<(), io::Error>, io::Error>,
}

impl Service for Client {
    type Req = Packet;
    type Resp = Vec<u8>;
    type Error = io::Error;
    type Fut = BoxFuture<Vec<u8>, io::Error>;

    fn call(&self, request: Packet) -> Self::Fut {
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
    use futures::{self, BoxFuture, Future};
    use protocol::{LOOP_HANDLE, TarpcTransport};
    use std::io;
    use std::net::SocketAddr;
    use super::Client;
    use take::Take;
    use tokio_core::TcpStream;
    use tokio_proto::pipeline;


    /// Types that can connect to a server asynchronously.
    pub trait Connect: Sized {
        /// The type of the future returned when calling connect.
        type Fut: Future<Item=Self, Error=io::Error>;

        /// Connects to a server located at the given address.
        fn connect(addr: &SocketAddr) -> Self::Fut;
    }

    /// A future that resolves to a `Client` or an `io::Error`.
    pub struct ClientFuture {
        inner: futures::Map<BoxFuture<TcpStream, io::Error>, fn(TcpStream) -> Client>,
    }

    impl Future for ClientFuture {
        type Item = Client;
        type Error = io::Error;

        fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
            self.inner.poll()
        }
    }

    impl Connect for Client {
        type Fut = ClientFuture;

        /// Starts an event loop on a thread and registers a new client
        /// connected to the given address.
        fn connect(addr: &SocketAddr) -> ClientFuture {
            fn connect(stream: TcpStream) -> Client {
                let loop_handle = LOOP_HANDLE.clone();
                let service = Take::new(move || Ok(TarpcTransport::new(stream)));
                Client { inner: pipeline::connect(loop_handle, service) }
            }
            ClientFuture {
                inner: LOOP_HANDLE.clone()
                                  .tcp_connect(addr)
                                  .map(connect)
            }
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

