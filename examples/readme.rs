// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(conservative_impl_trait, plugin)]
#![plugin(tarpc_plugins)]

extern crate futures;
#[macro_use]
extern crate tarpc;
extern crate tokio_core;

use futures::Future;
use tarpc::util::Never;
use tarpc::future::Connect;

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
    let mut core = tokio_core::reactor::Core::new().unwrap();
    let addr = HelloServer.listen("localhost:10000").unwrap();
    let f = FutureClient::connect(&addr)
        .map_err(tarpc::Error::from)
        .and_then(|client| {
            let resp1 = client.hello("Mom".to_string());
            let resp2 = client.hello("Dad".to_string());
            futures::collect(vec![resp1, resp2])
        }).map(|responses| {
            for resp in responses {
                println!("{}", resp);
            }
        });
    core.run(f).unwrap();
}
