// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(default_type_parameter_fallback, try_from)]
#[macro_use]
extern crate lazy_static;
extern crate mio;
#[macro_use]
extern crate tarpc;
extern crate env_logger;

use std::time;
use std::net;
use std::thread;
use std::io::{Read, Write};
use tarpc::{Client, Ctx};

lazy_static! {
    static ref BUF: Vec<u8> = gen_vec(CHUNK_SIZE as usize);
}

fn gen_vec(size: usize) -> Vec<u8> {
    let mut vec: Vec<u8> = Vec::with_capacity(size);
    for i in 0..size {
        vec.push((i % 1 << 8) as u8);
    }
    vec
}

service! {
    rpc read() -> Vec<u8>;
}

struct Server;

impl AsyncService for Server {
    fn read(&self, ctx: Ctx<Vec<u8>>) {
        ctx.ok(&*BUF).unwrap();
    }
}

const CHUNK_SIZE: u32 = 1 << 18;

fn bench_tarpc(target: u64) {
    let handle = Server.listen("localhost:0").unwrap();
    let client = SyncClient::connect(handle.local_addr()).unwrap();
    let start = time::Instant::now();
    let mut nread = 0;
    while nread < target {
        nread += client.read().unwrap().len() as u64;
        println!("Still reading...");
    }
    let duration = time::Instant::now() - start;
    println!("TARPC: {}MB/s",
             (target as f64 / (1024f64 * 1024f64)) /
             (duration.as_secs() as f64 + duration.subsec_nanos() as f64 / 10E9));
}

fn bench_tcp(target: u64) {
    let l = net::TcpListener::bind("localhost:0").unwrap();
    let addr = l.local_addr().unwrap();
    thread::spawn(move || {
        let (mut stream, _) = l.accept().unwrap();
        while let Ok(_) = stream.write_all(&*BUF) {
        }
    });
    let mut stream = net::TcpStream::connect(&addr).unwrap();
    let mut buf = vec![0; CHUNK_SIZE as usize];
    let start = time::Instant::now();
    let mut nread = 0;
    while nread < target {
        stream.read_exact(&mut buf[..]).unwrap();
        nread += CHUNK_SIZE as u64;
    }
    let duration = time::Instant::now() - start;
    println!("TCP:   {}MB/s",
             (target as f64 / (1024f64 * 1024f64)) /
             (duration.as_secs() as f64 + duration.subsec_nanos() as f64 / 10E9));
}

fn main() {
    let _ = env_logger::init();
    &*BUF; // to non-lazily initialize it.
    bench_tcp(256 << 20);
    bench_tarpc(256 << 20);
}
