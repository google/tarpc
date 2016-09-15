// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::serde::DeserializeError;
use errors::WireError;
use futures::{self, Async, Future};
use futures::stream::Empty;
use protocol::{LOOP_HANDLE, new_transport};
use serde::{Deserialize, Serialize};
use std::io;
use std::net::ToSocketAddrs;
use tokio_proto::pipeline;
use tokio_proto::server::{self, ServerHandle};
use tokio_service::NewService;
use util::Never;

/// A message from server to client.
pub type Response<T, E> = pipeline::Message<Result<T, WireError<E>>, Empty<Never, io::Error>>;

/// Spawns a service that binds to the given address and runs on the default tokio `Loop`.
pub fn listen_pipeline<A, S, Req, Resp, E>(addr: A, new_service: S) -> ListenFuture
    where S: NewService<Request = Result<Req, DeserializeError>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + Send + 'static,
          A: ToSocketAddrs,
          Req: Deserialize,
          Resp: Serialize,
          E: Serialize,
{
    // TODO(tikue): don't use ToSocketAddrs, or don't unwrap.
    let addr = addr.to_socket_addrs().unwrap().next().unwrap();

    let (tx, rx) = futures::oneshot();
    LOOP_HANDLE.spawn(move |handle| {
        Ok(tx.complete(server::listen(handle, addr, move |stream| {
                pipeline::Server::new(new_service.new_service()?, new_transport(stream))
            }).unwrap()))
    });
    ListenFuture { inner: rx }
}

/// A future that resolves to a `ServerHandle`.
pub struct ListenFuture {
    inner: futures::Oneshot<ServerHandle>,
}

impl Future for ListenFuture {
    type Item = ServerHandle;
    type Error = Never;

    fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
        match self.inner.poll().unwrap() {
            Async::Ready(server_handle) => Ok(Async::Ready(server_handle)),
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
