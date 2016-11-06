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
extern crate tokio_core;
extern crate futures_cpupool;

use futures::Future;
use futures_cpupool::{CpuFuture, CpuPool};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tarpc::future::{Connect};
use tarpc::util::{FirstSocketAddr, Never, spawn_core};
use tokio_core::reactor;

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
                futures::finished(vec)
            }))
    }
}

fn run_once(clients: Arc<Vec<FutureClient>>, concurrency: u32) -> Box<Future<Item=(), Error=()>>
{
    let start = Instant::now();
    let futs = clients.iter()
        .enumerate()
        .cycle()
        .enumerate()
        .take(concurrency as usize)
        .map(|(iteration, (client_id, client))| {
            let start = SystemTime::now();
            debug!("Client {} reading (iteration {})...", client_id, iteration);
            let future = client.read(CHUNK_SIZE).map(move |_| start.elapsed().unwrap());
            future
        })
        // Need an intermediate collection to kick off each future,
        // because futures::collect will iterate sequentially.
        .collect::<Vec<_>>();
    let futs = futures::collect(futs);

    Box::new(futs.map(move |latencies| {
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
    }).map_err(|e| panic!(e)))
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

    let server = Server::new().listen("localhost:0".first_socket_addr()).wait().unwrap();
    info!("Server listening on {}.", server.local_addr());

    // The driver of the main future.
    let mut core = reactor::Core::new().unwrap();

    let clients = (0..4)
        // Spin up a couple threads to drive the clients.
        .map(|i| (i, spawn_core()))
        .map(|(i, remote)| {
            info!("Client {} connecting...", i);
            FutureClient::connect_remotely(server.local_addr(), &remote)
                .map_err(|e| panic!(e))
        })
        // Need an intermediate collection to connect the clients in parallel,
        // because `futures::collect` iterates sequentially.
        .collect::<Vec<_>>();

    let runs = futures::collect(clients).and_then(|clients| {
        let clients = Arc::new(clients);
        let runs = (1...MAX_CONCURRENCY)
            .map(move |concurrency| run_once(clients.clone(), concurrency));
        futures::collect(runs)
    });

    info!("Starting...");
    core.run(runs).unwrap();
}
