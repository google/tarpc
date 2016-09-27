// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use REMOTE;
use bincode::serde::DeserializeError;
use errors::WireError;
use futures::{self, Async, Future};
use futures::stream::Empty;
use framed::Framed;
use serde::{Deserialize, Serialize};
use std::io;
use std::net::SocketAddr;
use tokio_core::reactor::Handle;
use tokio_proto::{self as proto, multiplex};
use tokio_proto::server::{self, ServerHandle};
use tokio_service::NewService;
use util::Never;

/// A message from server to client.
pub type Response<T, E> = proto::Message<Result<T, WireError<E>>, Empty<Never, io::Error>>;

/// Spawns a service that binds to the given address and runs on the default reactor core.
pub fn listen<S, Req, Resp, E>(addr: SocketAddr, new_service: S) -> ListenFuture
    where S: NewService<Request = Result<Req, DeserializeError>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + Send + 'static,
          Req: Deserialize,
          Resp: Serialize,
          E: Serialize,
{
    let (tx, rx) = futures::oneshot();
    REMOTE.spawn(move |handle| {
        Ok(tx.complete(listen_with(addr, new_service, handle)))
    });
    ListenFuture { inner: rx }
}

/// Spawns a service that binds to the given address using the given handle.
pub fn listen_with<S, Req, Resp, E>(addr: SocketAddr, new_service: S, handle: &Handle)
    -> io::Result<ServerHandle>
    where S: NewService<Request = Result<Req, DeserializeError>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + Send + 'static,
          Req: Deserialize,
          Resp: Serialize,
          E: Serialize,
{
    server::listen(handle, addr, move |stream| {
            Ok(multiplex::Server::new(new_service.new_service()?, Framed::new(stream)))
        })
}

/// A future that resolves to a `ServerHandle`.
pub struct ListenFuture {
    inner: futures::Oneshot<io::Result<ServerHandle>>,
}

impl Future for ListenFuture {
    type Item = ServerHandle;
    type Error = io::Error;

    fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
        // Can't panic the oneshot is always completed.
        match self.inner.poll().unwrap() {
            Async::Ready(result) => result.map(Async::Ready),
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
