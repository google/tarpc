// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(conservative_impl_trait, plugin)]
#![plugin(tarpc_plugins)]

extern crate env_logger;
#[macro_use]
extern crate tarpc;
extern crate futures;

use add::{FutureService as AddFutureService, FutureServiceExt as AddExt};
use double::{FutureService as DoubleFutureService, FutureServiceExt as DoubleExt};
use futures::{BoxFuture, Future};
use std::sync::{Arc, Mutex};
use tarpc::util::{FirstSocketAddr, Message, Never};
use tarpc::future::Connect as Fc;
use tarpc::sync::Connect as Sc;
use tarpc::{ClientConfig, ServerConfig};

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
    type AddFut = futures::Finished<i32, Never>;

    fn add(&self, x: i32, y: i32) -> Self::AddFut {
        futures::finished(x + y)
    }
}

#[derive(Clone)]
struct DoubleServer {
    client: Arc<Mutex<add::FutureClient>>,
}

impl DoubleServer {
    fn new(client: add::FutureClient) -> Self {
        DoubleServer {
            client: Arc::new(Mutex::new(client)),
        }
    }
}

impl DoubleFutureService for DoubleServer {
    type DoubleFut = BoxFuture<i32, Message>;

    fn double(&self, x: i32) -> Self::DoubleFut {
        self.client
            .lock()
            .unwrap()
            .add(x, x)
            .map_err(|e| e.to_string().into())
            .boxed()
    }
}

fn main() {
    let _ = env_logger::init();
    let add_addr = AddServer.listen("localhost:0".first_socket_addr(), ServerConfig::new_tcp()).wait().unwrap();
    let add_client = add::FutureClient::connect(&add_addr, ClientConfig::new_tcp()).wait().unwrap();

    let double = DoubleServer::new(add_client);
    let double_addr = double.listen("localhost:0".first_socket_addr(), ServerConfig::new_tcp()).wait().unwrap();

    let double_client = double::SyncClient::connect(&double_addr, ClientConfig::new_tcp()).unwrap();
    for i in 0..5 {
        println!("{:?}", double_client.double(i).unwrap());
    }
}
