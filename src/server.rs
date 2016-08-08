// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use futures::{self, IntoFuture};
use protocol::{REACTOR, TarpcTransport};
use protocol::writer::Packet;
use serde::Serialize;
use std::net::ToSocketAddrs;
use tokio::NewService;
use tokio::proto::pipeline;
use tokio::server::{self, ServerHandle};
use RpcError;

/// Sets up servers.
#[derive(Clone, Copy, Debug)]
pub struct Server;

impl Server {
    /// Start a Tarpc service listening on the given address.
    pub fn listen<A, T>(self, addr: A, new_service: T) -> ::Result<ServerHandle>
        where T: NewService<Req = Vec<u8>, Resp = Packet, Error = ::Error> + Send + 'static,
              A: ToSocketAddrs
    {
        let mut addrs = try!(addr.to_socket_addrs());
        let addr = if let Some(a) = addrs.next() {
            a
        } else {
            return Err(::Error::NoAddressFound);
        };

        server::listen(&REACTOR.lock().unwrap(), addr, move |stream| {
                let service = try!(new_service.new_service());
                pipeline::Server::new(service, TarpcTransport::new(stream))
            })
            .map_err(Into::into)
    }
}

#[doc(hidden)]
pub fn reply<T: Serialize>(result: Result<T, RpcError>) -> futures::Done<Packet, ::Error> {
    serialize_reply(result).into_future()
}

#[doc(hidden)]
#[inline]
pub fn serialize_reply<T: Serialize>(result: Result<T, RpcError>) -> ::Result<Packet> {
    let packet = try!(Packet::new(&result));
    Ok(packet)
}
