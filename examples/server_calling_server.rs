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
use double::{FutureService as DoubleService, FutureServiceExt as DoubleExt};
use tarpc::{Connect, Never, StringError};

pub mod add {
    service! {
        /// Add two ints together.
        rpc add(x: i32, y: i32) -> i32;
    }
}

pub mod double {
    service! {
        /// 2 * x
        rpc double(x: i32) -> i32 | ::tarpc::StringError;
    }
}

#[derive(Clone)]
struct AddServer;

impl AddService for AddServer {
    fn add(&self, x: i32, y: i32) -> tarpc::Future<i32, Never> {
        futures::finished(x + y).boxed()
    }
}

#[derive(Clone)]
struct DoubleServer {
    client: add::FutureClient,
}

impl DoubleService for DoubleServer {
    fn double(&self, x: i32) -> tarpc::Future<i32, StringError> {
        self.client
            .add(&x, &x)
            .map_err(|e| StringError::Err(e.to_string()))
            .boxed()
    }
}

fn main() {
    let add = AddServer.listen("localhost:0").unwrap();
    let add_client = add::FutureClient::connect(add.local_addr()).wait().unwrap();
    let double = DoubleServer { client: add_client };
    let double = double.listen("localhost:0").unwrap();

    let double_client = double::SyncClient::connect(double.local_addr()).wait().unwrap();
    for i in 0..5 {
        println!("{:?}", double_client.double(&i).unwrap());
    }
}
