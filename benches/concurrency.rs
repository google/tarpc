// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(default_type_parameter_fallback, test, try_from)]

extern crate env_logger;
#[macro_use]
extern crate log;
#[macro_use]
extern crate tarpc;
extern crate test;

use test::Bencher;
use std::sync::mpsc::channel;
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
    fn read(&mut self, ctx: Ctx<Vec<u8>>, size: u32) {
        ctx.reply(Ok(gen_vec(size as usize))).unwrap();
    }
}

const CHUNK_SIZE: u32 = 1 << 18;
const CONCURRENCY: u32 = 100;

#[bench]
fn concurrency(bencher: &mut Bencher) {
    let _ = env_logger::init();
    let server = Server.listen("localhost:0").unwrap();
    let client = FutureClient::connect(server.local_addr().unwrap()).unwrap();

    let mut count = 0;
    let (tx, rx) = channel();
    bencher.iter(|| {
        tx.send(client.read(&CHUNK_SIZE)).unwrap();
        count += 1;
        if count % CONCURRENCY == 0 {
            for _ in 0..CONCURRENCY {
                rx.recv().unwrap().get().unwrap();
            }
        }
    });
    drop(tx);
    for result in rx.iter() {
        result.get().unwrap();
        info!("Got result at the end.");
    }
}
