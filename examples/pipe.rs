// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(default_type_parameter_fallback)]
extern crate mio;
#[macro_use]
extern crate tarpc;

use mio::unix::pipe;
use tarpc::{Client, Ctx, Stream};

service! {
    rpc hey(s: String) -> String;
}

struct HeyServer;

impl AsyncService for HeyServer {
    fn hey(&mut self, ctx: Ctx<String>, s: String) {
        ctx.reply(Ok(format!("Hey, {}", s))).unwrap();
    }
}

fn main() {
    // TODO(tikue): Never actually going to accept any TCP connections. Should there be
    // another way to start a service if you don't want it to listen?
    let server = HeyServer.listen("localhost:0").unwrap();
    let (rx, tx) = pipe().unwrap();
    let (rx2, tx2) = pipe().unwrap();
    server.accept(Stream::Pipe(tx, rx2)).unwrap();
    let client = SyncClient::new(Stream::Pipe(tx2, rx)).unwrap();
    println!("{}", client.hey(&"Tim".to_string()).unwrap());
}
