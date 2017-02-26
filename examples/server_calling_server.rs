// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(plugin)]
#![plugin(tarpc_plugins)]

extern crate env_logger;
#[macro_use]
extern crate tarpc;
extern crate futures;
extern crate tokio_core;

use add::{FutureService as AddFutureService, FutureServiceExt as AddExt};
use double::{FutureService as DoubleFutureService, FutureServiceExt as DoubleExt};
use futures::{BoxFuture, Future, Stream};
use tarpc::future::{client, server};
use tarpc::future::client::ClientExt as Fc;
use tarpc::util::{FirstSocketAddr, Message, Never};
use tokio_core::reactor;

pub mod add {
    service! {
        /// Add two ints together.
        rpc add(x: i32, y: i32) -> i32;
    }
}

pub mod double {
    use tarpc::util::Message;

    service! {
        /// 2 * x
        rpc double(x: i32) -> i32 | Message;
    }
}

#[derive(Clone)]
struct AddServer;

impl AddFutureService for AddServer {
    type AddFut = Result<i32, Never>;

    fn add(&self, x: i32, y: i32) -> Self::AddFut {
        Ok(x + y)
    }
}

#[derive(Clone)]
struct DoubleServer {
    client: add::FutureClient,
}

impl DoubleServer {
    fn new(client: add::FutureClient) -> Self {
        DoubleServer { client: client }
    }
}

impl DoubleFutureService for DoubleServer {
    type DoubleFut = BoxFuture<i32, Message>;

    fn double(&self, x: i32) -> Self::DoubleFut {
        self.client
            .add(x, x)
            .map_err(|e| e.to_string().into())
            .boxed()
    }
}

fn main() {
    let _ = env_logger::init();
    let mut reactor = reactor::Core::new().unwrap();
    let (add, server) = AddServer.listen("localhost:0".first_socket_addr(),
                &reactor.handle(),
                server::Options::default())
        .unwrap();
    reactor.handle().spawn(server);

    let options = client::Options::default().handle(reactor.handle());
    let add_client = reactor.run(add::FutureClient::connect(add.addr(), options)).unwrap();

    let (double, server) = DoubleServer::new(add_client)
        .listen("localhost:0".first_socket_addr(),
                &reactor.handle(),
                server::Options::default())
        .unwrap();
    reactor.handle().spawn(server);

    let double_client =
        reactor.run(double::FutureClient::connect(double.addr(), client::Options::default()))
            .unwrap();
    reactor.run(futures::stream::futures_unordered((0..5).map(|i| double_client.double(i)))
            .map_err(|e| println!("{}", e))
            .for_each(|i| {
                println!("{:?}", i);
                Ok(())
            }))
        .unwrap();
}
