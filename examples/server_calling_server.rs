// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(conservative_impl_trait, plugin)]
#![plugin(snake_to_camel)]

#[macro_use]
extern crate tarpc;
extern crate futures;

use futures::{BoxFuture, Future};
use add::{FutureService as AddService, FutureServiceExt as AddExt};
use double::{FutureService as DoubleService, FutureServiceExt as DoubleExt};
use tarpc::util::{Never, Message};
use tarpc::future::Connect as Fc;
use tarpc::sync::Connect as Sc;

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

impl AddService for AddServer {
    type Add = futures::Finished<i32, Never>;

    fn add(&self, x: i32, y: i32) -> Self::Add {
        futures::finished(x + y)
    }
}

#[derive(Clone)]
struct DoubleServer {
    client: add::FutureClient,
}

impl DoubleService for DoubleServer {
    type Double = BoxFuture<i32, Message>;

    fn double(&self, x: i32) -> Self::Double {
        self.client
            .add(&x, &x)
            .map_err(|e| e.to_string().into())
            .boxed()
    }
}

fn main() {
    let add = AddServer.listen("localhost:0").unwrap();
    let add_client = add::FutureClient::connect(add.local_addr()).wait().unwrap();
    let double = DoubleServer { client: add_client };
    let double = double.listen("localhost:0").unwrap();

    let double_client = double::SyncClient::connect(double.local_addr()).unwrap();
    for i in 0..5 {
        println!("{:?}", double_client.double(&i).unwrap());
    }
}
