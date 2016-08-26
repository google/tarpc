// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use futures::{self, Future};
use futures::stream::BoxStream;
use futures_cpupool::{CpuFuture, CpuPool};
use protocol::{LOOP_HANDLE, TarpcTransport};
use protocol::writer::Packet;
use serde::Serialize;
use std::net::ToSocketAddrs;
use tokio_proto::proto::pipeline;
use tokio_proto::NewService;
use tokio_proto::server::{self, ServerHandle};

/// Sets up servers.
#[derive(Clone, Copy, Debug)]
pub struct Server;

impl Server {
    /// Start a Tarpc service listening on the given address.
    pub fn listen<A, T>(self, addr: A, new_service: T) -> ::Result<ServerHandle>
        where T: NewService<Req = Vec<u8>,
                            Resp = pipeline::Message<Packet, BoxStream<(), ::Error>>,
                            Error = ::Error> + Send + 'static,
              A: ToSocketAddrs
    {
        let mut addrs = try!(addr.to_socket_addrs());
        let addr = if let Some(a) = addrs.next() {
            a
        } else {
            return Err(::Error::NoAddressFound);
        };

        server::listen(LOOP_HANDLE.clone(), addr, move |stream| {
                let service = try!(new_service.new_service());
                pipeline::Server::new(service, TarpcTransport::new(stream))
            })
            .wait()
            .map_err(Into::into)
    }
}

/// Returns a future containing the serialized reply.
///
/// Because serialization can take a non-trivial
/// amount of cpu time, it is run on a thread pool.
#[doc(hidden)]
#[inline]
pub fn serialize_reply<T: Serialize + Send + 'static>(result: ::Result<T>) -> SerializeFuture {
    return POOL.execute(move || {
            let packet = try!(Packet::serialize(&result.map_err(::RpcError::from)));
            Ok(pipeline::Message::WithoutBody(packet))
        })
        .map_err(unreachable as fn(Box<::std::any::Any + Send>) -> ::Error)
        .flatten();

    fn unreachable(_err: Box<::std::any::Any + Send>) -> ::Error {
        unreachable!()
    }
}

#[doc(hidden)]
pub type SerializeFuture =
    futures::Flatten<futures::MapErr<CpuFuture<::Result<SerializedReply>>,
                                     fn(Box<::std::any::Any + Send>) -> ::Error>>;

#[doc(hidden)]
pub type SerializedReply = pipeline::Message<Packet, BoxStream<(), ::Error>>;

lazy_static! {
    static ref POOL: CpuPool = { CpuPool::new_num_cpus() };
}
