// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(default_type_parameter_fallback, test, try_from)]

extern crate chrono;
extern crate env_logger;
#[macro_use]
extern crate log;
#[macro_use]
extern crate tarpc;
extern crate test;

use chrono::Duration;
use std::time::Instant;
use tarpc::{Client, Ctx};

fn gen_vec(size: usize) -> Vec<u8> {
    let mut vec: Vec<u8> = Vec::with_capacity(size);
    for i in 0..size {
        vec.push((i % 1 << 8) as u8);
    }
    vec
}

service! {
    rpc read(size: u32) -> Vec<u8>;
}

struct Server;

impl AsyncService for Server {
    fn read(&self, ctx: Ctx<Vec<u8>>, size: u32) {
        ctx.reply(Ok(gen_vec(size as usize))).unwrap();
    }
}

const CHUNK_SIZE: u32 = 1 << 18;
const CONCURRENCY: u32 = 100;

fn main() {
    let _ = env_logger::init();
    let server = Server.listen("localhost:0").unwrap();
    let client = FutureClient::connect(server.local_addr().unwrap()).unwrap();

    let mut futures = Vec::with_capacity(CONCURRENCY as usize);
    let start = Instant::now();
    for _ in 0..CONCURRENCY {
        futures.push(client.read(&CHUNK_SIZE));
    }
    for future in futures {
        future.get().unwrap();
    }
    let duration = Instant::now() - start;
    println!("Mean time per request: {} µs",
             Duration::from_std(duration / CONCURRENCY).unwrap().num_microseconds().unwrap());
}
