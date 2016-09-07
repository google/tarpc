// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(inclusive_range_syntax, conservative_impl_trait, plugin)]
#![plugin(tarpc_plugins)]

extern crate chrono;
extern crate env_logger;
extern crate futures;
#[macro_use]
extern crate log;
#[macro_use]
extern crate tarpc;
extern crate futures_cpupool;

use futures::Future;
use futures_cpupool::{CpuFuture, CpuPool};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use tarpc::future::{Connect};
use tarpc::util::Never;

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
    type ReadFut = CpuFuture<Vec<u8>, Never>;

    fn read(&self, size: u32) -> Self::ReadFut {
        self.0
            .spawn(futures::lazy(move || {
                let mut vec: Vec<u8> = Vec::with_capacity(size as usize);
                for i in 0..size {
                    vec.push((i % 1 << 8) as u8);
                }
                futures::finished::<_, Never>(vec)
            }))
    }
}

fn run_once(clients: &[FutureClient], concurrency: u32, print: bool) {
    let _ = env_logger::init();

    let start = Instant::now();
    let futures: Vec<_> = clients.iter()
        .cycle()
        .take(concurrency as usize)
        .map(|client| {
            let start = SystemTime::now();
            let future = client.read(&CHUNK_SIZE).map(move |_| start.elapsed().unwrap());
            thread::yield_now();
            future
        })
        .collect();

    let latencies: Vec<_> = futures.into_iter()
        .map(|future| {
            future.wait().unwrap()
        })
        .collect();
    let total_time = start.elapsed();

    let sum_latencies = latencies.iter().fold(Duration::new(0, 0), |sum, &dur| sum + dur);
    let mean = sum_latencies / latencies.len() as u32;
    let min_latency = *latencies.iter().min().unwrap();
    let max_latency = *latencies.iter().max().unwrap();

    if print {
        println!("{} requests => Mean={}µs, Min={}µs, Max={}µs, Total={}µs",
                 latencies.len(),
                 mean.microseconds(),
                 min_latency.microseconds(),
                 max_latency.microseconds(),
                 total_time.microseconds());
    }
}

trait Microseconds {
    fn microseconds(&self) -> i64;
}

impl Microseconds for Duration {
    fn microseconds(&self) -> i64 {
         chrono::Duration::from_std(*self)
             .unwrap()
             .num_microseconds()
             .unwrap()
    }
}

const CHUNK_SIZE: u32 = 1 << 10;
const MAX_CONCURRENCY: u32 = 100;

fn main() {
    let _ = env_logger::init();
    let server = Server::new().listen("localhost:0").unwrap();
    println!("Server listening on {}.", server.local_addr());
    let clients: Vec<_> = (1...5)
        .map(|i| {
            println!("Client {} connecting...", i);
            FutureClient::connect(server.local_addr()).wait().unwrap()
        })
        .collect();
    println!("Starting...");

    run_once(&clients, MAX_CONCURRENCY, false);
    for concurrency in 1...MAX_CONCURRENCY {
        run_once(&clients, concurrency, true);
    }
}
