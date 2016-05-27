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

use mio::unix::pipe;
#[cfg(test)]
use self::test::Bencher;
use tarpc::{Client, Ctx};

service! {
    rpc ack();
}

struct Server;

impl AsyncService for Server {
    fn ack(&mut self, ctx: Ctx<()>) {
        ctx.reply(Ok(())).unwrap();
    }
}

#[cfg(test)]
#[bench]
fn latency(bencher: &mut Bencher) {
    let _ = env_logger::init();
    let server = Server.listen("localhost:0").unwrap();

    let (rx1, tx1) = pipe().unwrap();
    let (rx2, tx2) = pipe().unwrap();
    server.accept((tx1, rx2)).unwrap();
    let client = SyncClient::connect((tx2, rx1)).unwrap();

    bencher.iter(|| {
        client.ack().unwrap();
    });
}
