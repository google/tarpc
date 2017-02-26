// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

// required by `FutureClient` (not used directly in this example)
#![feature(plugin)]
#![plugin(tarpc_plugins)]

extern crate futures;
#[macro_use]
extern crate tarpc;
extern crate tokio_core;

use std::sync::mpsc;
use std::thread;
use tarpc::sync::{client, server};
use tarpc::sync::client::ClientExt;
use tarpc::util::Never;

service! {
    rpc hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl SyncService for HelloServer {
    fn hello(&self, name: String) -> Result<String, Never> {
        Ok(format!("Hello, {}!", name))
    }
}

fn main() {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let handle = HelloServer.listen("localhost:0", server::Options::default()).unwrap();
        tx.send(handle.addr()).unwrap();
        handle.run();
    });
    let client = SyncClient::connect(rx.recv().unwrap(), client::Options::default()).unwrap();
    println!("{}", client.hello("Mom".to_string()).unwrap());
}
