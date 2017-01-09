// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use {REMOTE, net2};
use bincode::serde::DeserializeError;
use errors::WireError;
use framed::Proto;
use futures::{self, Async, Future, Stream};
use serde::{Deserialize, Serialize};
use std::io;
use std::net::SocketAddr;
use tokio_core::net::TcpListener;
use tokio_core::reactor::Handle;
use tokio_proto::BindServer;
use tokio_service::NewService;

/// A message from server to client.
pub type Response<T, E> = Result<T, WireError<E>>;

/// Spawns a service that binds to the given address and runs on the default reactor core.
pub fn listen<S, Req, Resp, E>(addr: SocketAddr, new_service: S) -> ListenFuture
    where S: NewService<Request = Result<Req, DeserializeError>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + Send + 'static,
          Req: Deserialize + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    let (tx, rx) = futures::oneshot();
    REMOTE.spawn(move |handle| Ok(tx.complete(listen_with(addr, new_service, handle.clone()))));
    ListenFuture { inner: rx }
}

/// Spawns a service that binds to the given address using the given handle.
pub fn listen_with<S, Req, Resp, E>(addr: SocketAddr,
                                    new_service: S,
                                    handle: Handle)
                                    -> io::Result<SocketAddr>
    where S: NewService<Request = Result<Req, DeserializeError>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + Send + 'static,
          Req: Deserialize + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    let listener = listener(&addr, &handle)?;
    let addr = listener.local_addr()?;

    let handle2 = handle.clone();
    let server = listener.incoming().for_each(move |(socket, _)| {
        Proto::new().bind_server(&handle2, socket, new_service.new_service()?);

        Ok(())
    }).map_err(|e| error!("While processing incoming connections: {}", e));
    handle.spawn(server);
    Ok(addr)
}

fn listener(addr: &SocketAddr,
            handle: &Handle) -> io::Result<TcpListener> {
    const PENDING_CONNECTION_BACKLOG = 1024;

    match *addr {
        SocketAddr::V4(_) => net2::TcpBuilder::new_v4(),
        SocketAddr::V6(_) => net2::TcpBuilder::new_v6()
    }?
    .reuse_address(true)?
    .bind(addr)?
    .listen(PENDING_CONNECTION_BACKLOG)
    .and_then(|l| {
        TcpListener::from_listener(l, addr, handle)
    })
}

/// A future that resolves to a `ServerHandle`.
pub struct ListenFuture {
    inner: futures::Oneshot<io::Result<SocketAddr>>,
}

impl ListenFuture {
    #[doc(hidden)]
    pub fn from_oneshot(rx: futures::Oneshot<io::Result<SocketAddr>>) -> Self {
        ListenFuture { inner: rx }
    }
}

impl Future for ListenFuture {
    type Item = SocketAddr;
    type Error = io::Error;

    fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
        // Can't panic the oneshot is always completed.
        match self.inner.poll().unwrap() {
            Async::Ready(result) => result.map(Async::Ready),
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
