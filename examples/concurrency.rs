// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(inclusive_range_syntax, conservative_impl_trait)]

extern crate chrono;
extern crate env_logger;
extern crate futures;
#[macro_use]
extern crate log;
#[macro_use]
extern crate tarpc;
extern crate futures_cpupool;

use futures::Future;
use futures_cpupool::CpuPool;
use std::ops::Add;
use std::time::{Duration, Instant, SystemTime};
use tarpc::Connect;

service! {
    rpc read(size: u32) -> Vec<u8>;
}

#[derive(Clone)]
struct Server(CpuPool);

impl Server {
    fn new() -> Self {
        Server(CpuPool::new_num_cpus())
    }
}

impl FutureService for Server {
    fn read(&self, size: u32) -> tarpc::Future<Vec<u8>> {
        self.0
            .execute(move || {
                let mut vec: Vec<u8> = Vec::with_capacity(size as usize);
                for i in 0..size {
                    vec.push((i % 1 << 8) as u8);
                }
                vec
            })
            .map_err(|_| -> tarpc::Error { unreachable!() })
            .boxed()
    }
}

fn run_once(clients: &[FutureClient], concurrency: u32, print: bool) {
    let _ = env_logger::init();

    let start = Instant::now();
    let futures: Vec<_> = clients.iter()
        .cycle()
        .take(concurrency as usize)
        .map(|client| (client.read(&CHUNK_SIZE), SystemTime::now()))
        .collect();
    let sum_latencies: Duration = futures.into_iter()
        .map(|(future, start)| {
            future.wait().unwrap();
            SystemTime::now().duration_since(start).unwrap()
        })
        .fold(Duration::new(0, 0), Duration::add);

    let total_time = start.elapsed();
    if print {
        println!("Mean time per request: {} µs, Total time for {} requests: {} µs",
                 chrono::Duration::from_std(sum_latencies / concurrency)
                     .unwrap()
                     .num_microseconds()
                     .unwrap(),
                 concurrency,
                 chrono::Duration::from_std(total_time).unwrap().num_microseconds().unwrap());
    }
}

const CHUNK_SIZE: u32 = 1 << 10;
const MAX_CONCURRENCY: u32 = 100;

fn main() {
    let _ = env_logger::init();
    let server = Server::new().listen("localhost:0").unwrap();
    println!("Server listening.");
    let clients: Vec<_> = (1...5)
        .map(|_| FutureClient::connect(server.local_addr()).wait().unwrap())
        .collect();
    println!("Starting...");

    run_once(&clients, MAX_CONCURRENCY, false);
    for concurrency in 1...MAX_CONCURRENCY {
        run_once(&clients, concurrency, true);
    }
}
