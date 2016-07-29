// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(default_type_parameter_fallback, test, try_from)]

extern crate mio;
#[macro_use]
extern crate tarpc;
#[cfg(test)]
extern crate test;
extern crate env_logger;

#[cfg(test)]
use self::test::Bencher;
use tarpc::{Client, Ctx};

service! {
    rpc ack();
}

struct Server;

impl AsyncService for Server {
    fn ack(&self, ctx: Ctx<()>) {
        ctx.ok(()).unwrap();
    }
}

#[cfg(test)]
#[bench]
fn latency(bencher: &mut Bencher) {
    let _ = env_logger::init();
    let server = Server.listen("localhost:0").unwrap();
    let client = SyncClient::connect(server.local_addr()).unwrap();

    bencher.iter(|| {
        client.ack().unwrap();
    });
}
