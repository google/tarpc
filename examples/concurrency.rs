// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(inclusive_range_syntax, conservative_impl_trait, plugin)]
#![plugin(tarpc_plugins)]

extern crate chrono;
extern crate clap;
extern crate env_logger;
extern crate futures;
#[macro_use]
extern crate log;
#[macro_use]
extern crate tarpc;
extern crate tokio_core;
extern crate futures_cpupool;

use clap::{Arg, App};
use futures::Future;
use futures_cpupool::{CpuFuture, CpuPool};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};
use tarpc::future::{Connect};
use tarpc::util::{FirstSocketAddr, Never, spawn_core};
use tokio_core::reactor;

service! {
    rpc read(size: u32) -> Vec<u8>;
}

#[derive(Clone)]
struct Server {
    pool: CpuPool,
    request_count: Arc<AtomicUsize>,
}

impl Server {
    fn new() -> Self {
        Server {
            pool: CpuPool::new_num_cpus(),
            request_count: Arc::new(AtomicUsize::new(1)),
        }
    }
}

impl FutureService for Server {
    type ReadFut = CpuFuture<Vec<u8>, Never>;

    fn read(&self, size: u32) -> Self::ReadFut {
        let request_number = self.request_count.fetch_add(1, Ordering::SeqCst);
        debug!("Server received read({}) no. {}", size, request_number);
        self.pool
            .spawn(futures::lazy(move || {
                let mut vec: Vec<u8> = Vec::with_capacity(size as usize);
                for i in 0..size {
                    vec.push((i % 1 << 8) as u8);
                }
                debug!("Server sending response no. {}", request_number);
                futures::finished(vec)
            }))
    }
}

const CHUNK_SIZE: u32 = 1 << 10;

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

fn run_once(clients: Vec<FutureClient>, concurrency: u32) -> impl Future<Item=(), Error=()> {
    let start = Instant::now();
    let futs = clients.iter()
        .enumerate()
        .cycle()
        .enumerate()
        .take(concurrency as usize)
        .map(|(iteration, (client_id, client))| {
            let iteration = iteration + 1;
            let start = SystemTime::now();
            debug!("Client {} reading (iteration {})...", client_id, iteration);
            let future = client.read(CHUNK_SIZE).map(move |_| {
                let elapsed = start.elapsed().unwrap();
                debug!("Client {} received reply (iteration {}).", client_id, iteration);
                elapsed
            });
            future
        })
        // Need an intermediate collection to kick off each future,
        // because futures::collect will iterate sequentially.
        .collect::<Vec<_>>();
    let futs = futures::collect(futs);

    futs.map(move |latencies| {
        let total_time = start.elapsed();

        let sum_latencies = latencies.iter().fold(Duration::new(0, 0), |sum, &dur| sum + dur);
        let mean = sum_latencies / latencies.len() as u32;
        let min_latency = *latencies.iter().min().unwrap();
        let max_latency = *latencies.iter().max().unwrap();

        info!("{} requests => Mean={}µs, Min={}µs, Max={}µs, Total={}µs",
                 latencies.len(),
                 mean.microseconds(),
                 min_latency.microseconds(),
                 max_latency.microseconds(),
                 total_time.microseconds());
    }).map_err(|e| panic!(e))
}

fn main() {
    let _ = env_logger::init();
    let matches = App::new("Tarpc Concurrency")
                          .about("Demonstrates making concurrent requests to a tarpc service.")
                          .arg(Arg::with_name("concurrency")
                               .short("c")
                               .long("concurrency")
                               .value_name("LEVEL")
                               .help("Sets a custom concurrency level")
                               .takes_value(true))
                          .arg(Arg::with_name("clients")
                               .short("n")
                               .long("num_clients")
                               .value_name("AMOUNT")
                               .help("How many clients to distribute requests between")
                               .takes_value(true))
                          .get_matches();
    let concurrency = matches.value_of("concurrency")
        .map(&str::parse)
        .map(Result::unwrap)
        .unwrap_or(10);
    let num_clients = matches.value_of("clients")
        .map(&str::parse)
        .map(Result::unwrap)
        .unwrap_or(4);

    let addr = Server::new().listen("localhost:0".first_socket_addr()).wait().unwrap();
    info!("Server listening on {}.", addr);

    let clients = (0..num_clients)
        // Spin up a couple threads to drive the clients.
        .map(|i| (i, spawn_core()))
        .map(|(i, remote)| {
            info!("Client {} connecting...", i);
            FutureClient::connect_remotely(&addr, &remote)
                .map_err(|e| panic!(e))
        })
        // Need an intermediate collection to connect the clients in parallel,
        // because `futures::collect` iterates sequentially.
        .collect::<Vec<_>>();

    let run = futures::collect(clients).and_then(|clients| run_once(clients, concurrency));

    info!("Starting...");

    // The driver of the main future.
    let mut core = reactor::Core::new().unwrap();

    core.run(run).unwrap();
}
