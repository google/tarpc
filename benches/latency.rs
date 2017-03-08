// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(plugin, test)]
#![plugin(tarpc_plugins)]

#[macro_use]
extern crate tarpc;
#[cfg(test)]
extern crate test;
extern crate env_logger;
extern crate futures;
extern crate tokio_core;

use tarpc::future::{client, server};
use tarpc::future::client::ClientExt;
use tarpc::util::{FirstSocketAddr, Never};
#[cfg(test)]
use test::Bencher;
use tokio_core::reactor;

service! {
    rpc ack();
}

#[derive(Clone)]
struct Server;

impl FutureService for Server {
    type AckFut = futures::Finished<(), Never>;
    fn ack(&self) -> Self::AckFut {
        futures::finished(())
    }
}

#[cfg(test)]
#[bench]
fn latency(bencher: &mut Bencher) {
    let _ = env_logger::init();
    let mut reactor = reactor::Core::new().unwrap();
    let (handle, server) = Server.listen("localhost:0".first_socket_addr(),
                &reactor.handle(),
                server::Options::default())
        .unwrap();
    reactor.handle().spawn(server);
    let client = FutureClient::connect(handle.addr(),
                                       client::Options::default().handle(reactor.handle()));
    let client = reactor.run(client).unwrap();

    bencher.iter(|| reactor.run(client.ack()).unwrap());
}
