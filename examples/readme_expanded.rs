// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(conservative_impl_trait, plugin, proc_macro)]
#![plugin(tarpc_plugins)]

extern crate bincode;
extern crate env_logger;
extern crate futures;
#[macro_use]
extern crate log;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate tarpc;
extern crate tokio_core;
extern crate tokio_service;

use bincode::serde::DeserializeError;
use futures::Future;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::thread;
use tarpc::WireError;
use tarpc::future::Connect;
use tarpc::util::FirstSocketAddr;
use tarpc::util::Never;
use tokio_core::reactor::{Handle, Remote};
use tokio_service::Service;

#[derive(Debug, Serialize, Deserialize)]
enum Request {
    Hello(String),
}

#[derive(Debug, Serialize, Deserialize)]
enum Response {
    Hello(String),
}

#[derive(Debug, Serialize, Deserialize)]
enum Error {
    Hello(Never),
}

/// Defines the `Future` RPC service. Implementors must be `Clone`, `Send`, and `'static`,
/// as required by `tokio_proto::NewService`. This is required so that the service can be used
/// to respond to multiple requests concurrently.
pub trait FutureService: Send + Clone + 'static {
    type HelloFut: Future<Item = String, Error = Never>;
    fn hello(&self, name: String) -> Self::HelloFut;
}

/// Provides a function for starting the service. This is a separate trait from
/// `FutureService` to prevent collisions with the names of RPCs.
pub trait FutureServiceExt: FutureService {
    fn listen(self, addr: SocketAddr) -> tarpc::ListenFuture {
        let (tx, rx) = futures::oneshot();
        tarpc::REMOTE.spawn(move |handle| Ok(tx.complete(Self::listen_with(self, addr, handle.clone()))));
        tarpc::ListenFuture::from_oneshot(rx)
    }

    /// Spawns the service, binding to the given address and running on the default tokio `Loop`.
    fn listen_with(self, addr: SocketAddr, handle: Handle) -> io::Result<SocketAddr> {
        return tarpc::listen_with(addr, move || Ok(AsyncServer(self.clone())), handle);

        #[derive(Clone, Debug)]
        struct AsyncServer<S>(S);

        type Fut = futures::Finished<tarpc::Response<Response, Error>, io::Error>;

        enum FutureReply<S: FutureService> {
            DeserializeError(Fut),
            Hello(futures::Then<S::HelloFut, Fut, fn(Result<String, Never>) -> Fut>),
        }

        impl<S: FutureService> Future for FutureReply<S> {
            type Item = tarpc::Response<Response, Error>;
            type Error = io::Error;

            fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
                match *self {
                    FutureReply::DeserializeError(ref mut future) => future.poll(),
                    FutureReply::Hello(ref mut future) => future.poll(),
                }
            }
        }

        impl<S> Service for AsyncServer<S>
            where S: FutureService
        {
            type Request = Result<Request, DeserializeError>;
            type Response = tarpc::Response<Response, Error>;
            type Error = io::Error;
            type Future = FutureReply<S>;

            fn call(&self, request: Self::Request) -> Self::Future {
                let request = match request {
                    Ok(request) => request,
                    Err(deserialize_err) => {
                        let err = Err(WireError::ServerDeserialize(deserialize_err.to_string()));
                        return FutureReply::DeserializeError(futures::finished(err));
                    }
                };

                match request {
                    Request::Hello(name) => {
                        fn wrap(response: Result<String, Never>) -> Fut {
                            let fut = response.map(Response::Hello)
                                .map_err(|error| WireError::App(Error::Hello(error)));
                            futures::finished(fut)
                        }
                        return FutureReply::Hello(self.0.hello(name).then(wrap));
                    }
                }
            }
        }
    }
}

/// Defines the blocking RPC service. Must be `Clone`, `Send`, and `'static`,
/// as required by `tokio_proto::NewService`. This is required so that the service can be used
/// to respond to multiple requests concurrently.
pub trait SyncService: Send + Clone + 'static {
    fn hello(&self, name: String) -> Result<String, Never>;
}

/// Provides a function for starting the service. This is a separate trait from
/// `SyncService` to prevent collisions with the names of RPCs.
pub trait SyncServiceExt: SyncService {
    fn listen<L>(self, addr: L) -> io::Result<SocketAddr>
        where L: ToSocketAddrs
    {
        let addr = addr.try_first_socket_addr()?;
        let (tx, rx) = futures::oneshot();
        tarpc::REMOTE.spawn(move |handle| {
            Ok(tx.complete(Self::listen_with(self, addr, handle.clone())))
        });
        tarpc::ListenFuture::from_oneshot(rx).wait()
    }

    /// Spawns the service, binding to the given address and running on
    /// the default tokio `Loop`.
    fn listen_with<L>(self, addr: L, handle: Handle) -> io::Result<SocketAddr>
        where L: ToSocketAddrs
    {
        let service = SyncServer { service: self };
        return service.listen_with(addr.try_first_socket_addr()?, handle);

        #[derive(Clone)]
        struct SyncServer<S> {
            service: S,
        }

        impl<S> FutureService for SyncServer<S>
            where S: SyncService
        {
            type HelloFut = futures::Flatten<futures::MapErr<futures::Oneshot<futures::Done<String, Never>>, fn(futures::Canceled) -> Never>>;

            fn hello(&self, name: String) -> Self::HelloFut {
                fn unimplemented(_: futures::Canceled) -> Never {
                    unimplemented!()
                }

                let (complete, promise) = futures::oneshot();
                let service = self.clone();
                const UNIMPLEMENTED: fn(futures::Canceled) -> Never = unimplemented;
                thread::spawn(move || {
                    let reply = SyncService::hello(&service.service, name);
                    complete.complete(futures::IntoFuture::into_future(reply));
                });
                promise.map_err(UNIMPLEMENTED).flatten()
            }
        }
    }
}
impl<A> FutureServiceExt for A where A: FutureService {}
impl<S> SyncServiceExt for S where S: SyncService {}

type Client = tarpc::Client<Request, Response, Error>;

/// Implementation detail: Pending connection.
pub struct ConnectFuture<T> {
    inner: futures::Map<tarpc::ConnectFuture<Request, Response, Error>, fn(Client) -> T>,
}

impl<T> Future for ConnectFuture<T> {
    type Item = T;
    type Error = io::Error;

    fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
        self.inner.poll()
    }
}

/// Implementation detail: Pending connection.
pub struct ConnectWithFuture<'a, T> {
    inner: futures::Map<tarpc::ConnectWithFuture<'a, Request, Response, Error>, fn(Client) -> T>,
}

impl<'a, T> Future for ConnectWithFuture<'a, T> {
    type Item = T;
    type Error = io::Error;
    fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
        self.inner.poll()
    }
}

/// The client stub that makes RPC calls to the server. Exposes a Future interface.
#[derive(Debug)]
pub struct FutureClient(Client);

impl<'a> tarpc::future::Connect<'a> for FutureClient {
    type ConnectFut = ConnectFuture<Self>;
    type ConnectWithFut = ConnectWithFuture<'a, Self>;

    fn connect_remotely(addr: &SocketAddr, remote: &Remote) -> Self::ConnectFut {
        let client = Client::connect_remotely(addr, remote);
        ConnectFuture { inner: client.map(FutureClient) }
    }

    fn connect_with(addr: &SocketAddr, handle: &'a Handle) -> Self::ConnectWithFut {
        let client = Client::connect_with(addr, handle);
        ConnectWithFuture { inner: client.map(FutureClient) }
    }
}

impl FutureClient {
    pub fn hello(&self, name: String)
        -> impl Future<Item = String, Error = tarpc::Error<Never>> + 'static
    {
        let request = Request::Hello(name);

        self.0.call(request).then(move |msg| {
            match msg? {
                Ok(Response::Hello(msg)) => Ok(msg),
                Err(err) => {
                    Err(match err {
                        tarpc::Error::App(Error::Hello(err)) => tarpc::Error::App(err),
                        tarpc::Error::ServerDeserialize(err) => {
                            tarpc::Error::ServerDeserialize(err)
                        }
                        tarpc::Error::ServerSerialize(err) => tarpc::Error::ServerSerialize(err),
                        tarpc::Error::ClientDeserialize(err) => {
                            tarpc::Error::ClientDeserialize(err)
                        }
                        tarpc::Error::ClientSerialize(err) => tarpc::Error::ClientSerialize(err),
                        tarpc::Error::Io(error) => tarpc::Error::Io(error),
                    })
                }
            }
        })
    }
}

#[derive(Clone)]
struct HelloServer;

impl SyncService for HelloServer {
    fn hello(&self, name: String) -> Result<String, Never> {
        info!("Got request: {}", name);
        Ok(format!("Hello, {}!", name))
    }
}

fn main() {
    let _ = env_logger::init();
    let mut core = tokio_core::reactor::Core::new().unwrap();
    let addr = HelloServer.listen("localhost:10000").unwrap();
    let f = FutureClient::connect(&addr)
        .map_err(tarpc::Error::from)
        .and_then(|client| {
            let resp1 = client.hello("Mom".to_string());
            info!("Sent first request.");

            let resp2 = client.hello("Dad".to_string());
            info!("Sent second request.");

            futures::collect(vec![resp1, resp2])
        })
        .map(|responses| {
            for resp in responses {
                println!("{}", resp);
            }
        });
    core.run(f).unwrap();
}
