// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use errors::{SerializableError, WireError};
use futures::{self, Future};
use futures::stream::Empty;
use futures_cpupool::{CpuFuture, CpuPool};
use protocol::{LOOP_HANDLE, TarpcTransport};
use protocol::writer::Packet;
use serde::Serialize;
use std::io;
use std::net::ToSocketAddrs;
use tokio_proto::pipeline;
use tokio_proto::NewService;
use tokio_proto::server::{self, ServerHandle};

/// Start a Tarpc service listening on the given address.
pub fn listen<A, T>(addr: A, new_service: T) -> io::Result<ServerHandle>
    where T: NewService<Req = Vec<u8>,
                        Resp = pipeline::Message<Packet, Empty<(), io::Error>>,
                        Error = io::Error> + Send + 'static,
          A: ToSocketAddrs
{
    let mut addrs = addr.to_socket_addrs()?;
    let addr = if let Some(a) = addrs.next() {
        a
    } else {
        return Err(io::Error::new(io::ErrorKind::AddrNotAvailable,
                                  "`ToSocketAddrs::to_socket_addrs` returned an empty iterator."));
    };

    server::listen(LOOP_HANDLE.clone(), addr, move |stream| {
            pipeline::Server::new(new_service.new_service()?, TarpcTransport::new(stream))
        })
        .wait()
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
    POOL.spawn(futures::lazy(move || {
            let packet = match Packet::serialize(&result) {
                Ok(packet) => packet,
                Err(e) => {
                    let err: Result<T, WireError<E>> =
                        Err(WireError::ServerSerialize(e.to_string()));
                    Packet::serialize(&err).unwrap()
                }
            };
            futures::finished(pipeline::Message::WithoutBody(packet))
        }))
}

#[doc(hidden)]
pub type SerializeFuture = CpuFuture<SerializedReply, io::Error>;

#[doc(hidden)]
pub type SerializedReply = pipeline::Message<Packet, Empty<(), io::Error>>;

lazy_static! {
    static ref POOL: CpuPool = { CpuPool::new_num_cpus() };
}
