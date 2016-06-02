// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(default_type_parameter_fallback, try_from)]
#[macro_use]
extern crate tarpc;

use tarpc::{Client, RpcResult};

service! {
    rpc hello(name: String) -> String;
}

struct HelloServer;

impl SyncService for HelloServer {
    fn hello(&self, name: String) -> RpcResult<String> {
        Ok(format!("Hello, {}!", name))
    }
}

fn main() {
    let addr = "localhost:10000";
    let _server = HelloServer.listen(addr).unwrap();
    let client = SyncClient::connect(addr).unwrap();
    assert_eq!("Hello, Mom!", client.hello(&"Mom".to_string()).unwrap());
}
