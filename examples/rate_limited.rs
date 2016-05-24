// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(default_type_parameter_fallback)]

extern crate chrono;
extern crate env_logger;
#[macro_use]
extern crate log;
#[macro_use]
extern crate tarpc;

use chrono::Local;
use env_logger::LogBuilder;
use log::LogRecord;
use tarpc::{server, Client, RpcResult};
use std::env;
use std::sync::mpsc::channel;
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
    let format = |record: &LogRecord| {
            format!("{} - {} - {}",
                Local::now(),
                record.level(),
                record.args()
            )
        };

    let mut builder = LogBuilder::new();
    builder.format(format);
    if env::var("RUST_LOG").is_ok() {
        builder.parse(&env::var("RUST_LOG").unwrap());
    }
    builder.init().unwrap();
    let server = SleepServer.register("localhost:0", server::Config::max_requests(2)).unwrap();
    let client = AsyncClient::connect(server.local_addr()).unwrap();
    let total_requests = 10;
    let chans = (0..total_requests).map(|i| {
        let (tx, rx) = channel();
        client.sleep(move |_| {
            info!("{}", i);
            tx.send(()).unwrap();
        }, &1000).unwrap();
        rx
    }).collect::<Vec<_>>();
    for rx in chans { rx.recv().unwrap() }
}
