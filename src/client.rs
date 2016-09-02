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

/// A thin wrapper around `pipeline::Client` that handles Serialization.
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
    use futures::{BoxFuture, Future, IntoFuture};
    use protocol::{LOOP_HANDLE, TarpcTransport};
    use std::io;
    use std::net::ToSocketAddrs;
    use super::Client;
    use take::Take;
    use tokio_proto::pipeline;


    /// Types that can connect to a server asynchronously.
    pub trait Connect: Sized {
        /// Connects to a server located at the given address.
        fn connect<A>(addr: A) -> BoxFuture<Self, io::Error> where A: ToSocketAddrs;
    }

    impl Connect for Client {
        /// Starts an event loop on a thread and registers a new client
        /// connected to the given address.
        fn connect<A>(addr: A) -> BoxFuture<Self, io::Error>
            where A: ToSocketAddrs
        {
            let mut addrs = match addr.to_socket_addrs() {
                Ok(addrs) => addrs,
                Err(e) => return Err(e.into()).into_future().boxed(),
            };
            let addr = if let Some(a) = addrs.next() {
                a
            } else {
                return Err(io::Error::new(io::ErrorKind::AddrNotAvailable,
                                          "`ToSocketAddrs::to_socket_addrs` returned an empty \
                                           iterator."))
                    .into_future()
                    .boxed();
            };
            LOOP_HANDLE.clone()
                .tcp_connect(&addr)
                .map(|stream| {
                    let client = pipeline::connect(LOOP_HANDLE.clone(),
                                                   Take::new(move || Ok(TarpcTransport::new(stream))));
                    Client { inner: client }
                })
                .map_err(Into::into)
                .boxed()
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
            super::future::Connect::connect(addr).wait()
        }
    }
}

