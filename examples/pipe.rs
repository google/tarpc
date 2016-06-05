// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(default_type_parameter_fallback, try_from)]
extern crate mio;
#[macro_use]
extern crate tarpc;

#[cfg(unix)]
use mio::unix::pipe;
#[cfg(unix)]
use tarpc::{Client, Ctx};

service! {
    rpc hey(s: String) -> String;
}

#[cfg(unix)]
struct HeyServer;

#[cfg(unix)]
impl AsyncService for HeyServer {
    fn hey(&self, ctx: Ctx<String>, s: String) {
        ctx.ok(format!("Hey, {}", s)).unwrap();
    }
}

#[cfg(unix)]
fn main() {
    // TODO(tikue): Never actually going to accept any TCP connections. Should there be
    // another way to start a service if you don't want it to listen?
    let server = HeyServer.listen("localhost:0").unwrap();
    let (rx, tx) = pipe().unwrap();
    let (rx2, tx2) = pipe().unwrap();
    server.accept((tx, rx2)).unwrap();
    let client = SyncClient::connect((tx2, rx)).unwrap();
    println!("{}", client.hey(&"Tim".to_string()).unwrap());
}

#[cfg(not(unix))]
fn main() {}
