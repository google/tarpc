// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

// required by `FutureClient` (not used directly in this example)
#![feature(conservative_impl_trait, plugin)]
#![plugin(tarpc_plugins)]

extern crate futures;
#[macro_use]
extern crate tarpc;

use tarpc::util::Never;
use tarpc::sync::Connect;

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
    let addr = "localhost:10000";
    HelloServer.listen(addr, tarpc::ServerConfig::new_tcp()).unwrap();
    let client = SyncClient::connect(addr, tarpc::ClientConfig::new_tcp()).unwrap();
    println!("{}", client.hello("Mom".to_string()).unwrap());
}
