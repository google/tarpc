// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(default_type_parameter_fallback)]

extern crate env_logger;
#[macro_use]
extern crate tarpc;

use std::thread;
use tarpc::RpcResult;

service! {
    rpc hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl SyncService for HelloServer {
    fn hello(&self, name: String) -> RpcResult<String> {
        println!("Name: {}", name);
        Ok(format!("Hello, {}!", name))
    }
}

fn main() {
    extern crate bincode;
    env_logger::init().unwrap();
    println!("{:?}", bincode::serde::serialize(&"hi".to_string(), bincode::SizeLimit::Infinite).unwrap());
    let addr = "localhost:10000";
    let _server = HelloServer.listen(addr).unwrap();
    thread::park();
    println!("Done.");
}
