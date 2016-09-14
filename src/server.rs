// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use errors::{SerializableError, WireError};
use futures::{self, Async, Future};
use futures::stream::Empty;
use protocol::{LOOP_HANDLE, TarpcTransport};
use protocol::writer::Packet;
use serde::Serialize;
use std::io;
use std::net::ToSocketAddrs;
use tokio_proto::pipeline;
use tokio_proto::server::{self, ServerHandle};
use tokio_service::NewService;
use util::Never;

/// Spawns a service that binds to the given address and runs on the default tokio `Loop`.
pub fn listen<A, T>(addr: A, new_service: T) -> ListenFuture
    where T: NewService<Request = Vec<u8>,
                        Response = pipeline::Message<Packet, Empty<Never, io::Error>>,
                        Error = io::Error> + Send + 'static,
          A: ToSocketAddrs
{
    // TODO(tikue): don't use ToSocketAddrs, or don't unwrap.
    let addr = addr.to_socket_addrs().unwrap().next().unwrap();

    let (tx, rx) = futures::oneshot();
    LOOP_HANDLE.spawn(move |handle| {
        Ok(tx.complete(server::listen(handle, addr, move |stream| {
                pipeline::Server::new(new_service.new_service()?, TarpcTransport::new(stream))
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

/// Returns a future containing the serialized reply.
///
/// Because serialization can take a non-trivial
/// amount of cpu time, it is run on a thread pool.
#[doc(hidden)]
#[inline]
pub fn serialize_reply<T: Serialize + Send + 'static,
                       E: SerializableError>(result: Result<T, WireError<E>>)
                       -> SerializeFuture
{
    let packet = match Packet::serialize(&result) {
        Ok(packet) => packet,
        Err(e) => {
            let err: Result<T, WireError<E>> = Err(WireError::ServerSerialize(e.to_string()));
            Packet::serialize(&err).unwrap()
        }
    };
    futures::finished(pipeline::Message::WithoutBody(packet))
}

#[doc(hidden)]
pub type SerializeFuture = futures::Finished<SerializedReply, io::Error>;

#[doc(hidden)]
pub type SerializedReply = pipeline::Message<Packet, Empty<Never, io::Error>>;
