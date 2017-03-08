// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(plugin)]
#![plugin(tarpc_plugins)]

#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate tarpc;
extern crate env_logger;
extern crate futures;
extern crate serde;
extern crate tokio_core;

use std::io::{Read, Write, stdout};
use std::net;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time;
use tarpc::future::server;
use tarpc::sync::client::{self, ClientExt};
use tarpc::util::{FirstSocketAddr, Never};
use tokio_core::reactor;

lazy_static! {
    static ref BUF: Arc<serde::bytes::ByteBuf> = Arc::new(gen_vec(CHUNK_SIZE as usize).into());
}

fn gen_vec(size: usize) -> Vec<u8> {
    let mut vec: Vec<u8> = Vec::with_capacity(size);
    for i in 0..size {
        vec.push(((i % 2) << 8) as u8);
    }
    vec
}

service! {
    rpc read() -> Arc<serde::bytes::ByteBuf>;
}

#[derive(Clone)]
struct Server;

impl FutureService for Server {
    type ReadFut = Result<Arc<serde::bytes::ByteBuf>, Never>;

    fn read(&self) -> Self::ReadFut {
        Ok(BUF.clone())
    }
}

const CHUNK_SIZE: u32 = 1 << 19;

fn bench_tarpc(target: u64) {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reactor = reactor::Core::new().unwrap();
        let (addr, server) = Server.listen("localhost:0".first_socket_addr(),
                    &reactor.handle(),
                    server::Options::default())
            .unwrap();
        tx.send(addr).unwrap();
        reactor.run(server).unwrap();
    });
    let client = SyncClient::connect(rx.recv().unwrap().addr(), client::Options::default())
        .unwrap();
    let start = time::Instant::now();
    let mut nread = 0;
    while nread < target {
        nread += client.read().unwrap().len() as u64;
        print!(".");
        stdout().flush().unwrap();
    }
    println!("done");
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
        while let Ok(_) = stream.write_all(&*BUF) {}
    });
    let mut stream = net::TcpStream::connect(&addr).unwrap();
    let mut buf = vec![0; CHUNK_SIZE as usize];
    let start = time::Instant::now();
    let mut nread = 0;
    while nread < target {
        stream.read_exact(&mut buf[..]).unwrap();
        nread += CHUNK_SIZE as u64;
        print!(".");
        stdout().flush().unwrap();
    }
    println!("done");
    let duration = time::Instant::now() - start;
    println!("TCP:   {}MB/s",
             (target as f64 / (1024f64 * 1024f64)) /
             (duration.as_secs() as f64 + duration.subsec_nanos() as f64 / 10E9));
}

fn main() {
    let _ = env_logger::init();
    let _ = *BUF; // To non-lazily initialize it.
    bench_tcp(256 << 20);
    bench_tarpc(256 << 20);
}
