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

use add::{SyncService as AddSyncService, SyncServiceExt as AddExt};
use double::{SyncService as DoubleSyncService, SyncServiceExt as DoubleExt};
use std::sync::mpsc;
use std::thread;
use tarpc::sync::{client, server};
use tarpc::sync::client::ClientExt as Fc;
use tarpc::util::{FirstSocketAddr, Message, Never};

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

impl AddSyncService for AddServer {
    fn add(&self, x: i32, y: i32) -> Result<i32, Never> {
        Ok(x + y)
    }
}

#[derive(Clone)]
struct DoubleServer {
    client: add::SyncClient,
}

impl DoubleServer {
    fn new(client: add::SyncClient) -> Self {
        DoubleServer { client: client }
    }
}

impl DoubleSyncService for DoubleServer {
    fn double(&self, x: i32) -> Result<i32, Message> {
        self.client
            .add(x, x)
            .map_err(|e| e.to_string().into())
    }
}

fn main() {
    let _ = env_logger::init();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let handle = AddServer.listen("localhost:0".first_socket_addr(),
                    server::Options::default())
            .unwrap();
        tx.send(handle.addr()).unwrap();
        handle.run();
    });


    let add = rx.recv().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let add_client = add::SyncClient::connect(add, client::Options::default()).unwrap();
        let handle = DoubleServer::new(add_client)
            .listen("localhost:0".first_socket_addr(),
                    server::Options::default())
            .unwrap();
        tx.send(handle.addr()).unwrap();
        handle.run();
    });
    let double = rx.recv().unwrap();

    let double_client = double::SyncClient::connect(double, client::Options::default()).unwrap();
    for i in 0..5 {
        let doubled = double_client.double(i).unwrap();
        println!("{:?}", doubled);
    }
}
