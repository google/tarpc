// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(default_type_parameter_fallback)]
#[macro_use]
extern crate log;
#[macro_use]
extern crate tarpc;
use tarpc::{server, Client, RpcResult};
use std::thread;
use std::time::Duration;

service! {
    rpc sleep(millis: u64);
}

#[derive(Clone, Debug)]
struct SleepServer;

impl SyncService for SleepServer {
    fn sleep(&self, millis: u64) -> RpcResult<()> {
        thread::sleep(Duration::from_millis(millis));
        Ok(())
    }
}

fn main() {
    let server = SleepServer.register("localhost:0", server::Config::max_requests(Some(1))).unwrap();
    let client = AsyncClient::connect(server.local_addr()).unwrap();
    let total_requests = 10;
    for i in 0..total_requests {
        client.sleep(move |_| println!("{}", i), &100).unwrap();
    }
    thread::sleep(Duration::from_secs(total_requests));
}
