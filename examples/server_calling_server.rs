// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(conservative_impl_trait)]

#[macro_use]
extern crate tarpc;
extern crate futures;

use futures::Future;
use add::{FutureService as AddService, FutureServiceExt as AddExt};
use add_one::{FutureService as AddOneService, FutureServiceExt as AddOneExt};
use tarpc::Connect;

pub mod add {
    service! {
        /// Add two ints together.
        rpc add(x: i32, y: i32) -> i32;
    }
}

pub mod add_one {
    service! {
        /// 2 * (x + 1)
        rpc add_one(x: i32) -> i32;
    }
}

#[derive(Clone)]
struct AddServer;

impl AddService for AddServer {
    fn add(&self, x: i32, y: i32) -> tarpc::Future<i32> {
        futures::finished(x + y).boxed()
    }
}

#[derive(Clone)]
struct AddOneServer {
    client: add::FutureClient,
}

impl AddOneService for AddOneServer {
    fn add_one(&self, x: i32) -> tarpc::Future<i32> {
        self.client
            .add(&x, &1)
            .join(self.client.add(&x, &1))
            .then(|result| {
                let (x, y) = try!(result);
                Ok(x + y)
            })
            .boxed()
    }
}

fn main() {
    let add = AddServer.listen("localhost:0").unwrap();
    let add_client = add::FutureClient::connect(add.local_addr()).wait().unwrap();
    let add_one = AddOneServer { client: add_client };
    let add_one = add_one.listen("localhost:0").unwrap();

    let add_one_client = add_one::SyncClient::connect(add_one.local_addr()).wait().unwrap();
    for i in 0..5 {
        println!("{:?}", add_one_client.add_one(&i).unwrap());
    }
}
