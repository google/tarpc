// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(inclusive_range_syntax, conservative_impl_trait, plugin, never_type)]
#![plugin(tarpc_plugins)]

extern crate chrono;
extern crate clap;
extern crate env_logger;
extern crate futures;
#[macro_use]
extern crate log;
extern crate serde;
#[macro_use]
extern crate tarpc;
extern crate tokio_core;
extern crate futures_cpupool;

use clap::{Arg, App};
use futures::{Future, Stream};
use futures_cpupool::{CpuFuture, CpuPool};
use std::{cmp, thread};
use std::sync::{Arc, mpsc};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tarpc::future::{client, server};
use tarpc::future::client::ClientExt;
use tarpc::util::{FirstSocketAddr, Never};
use tokio_core::reactor;

service! {
    rpc read(size: u32) -> serde::bytes::ByteBuf;
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
    type ReadFut = CpuFuture<serde::bytes::ByteBuf, Never>;

    fn read(&self, size: u32) -> Self::ReadFut {
        let request_number = self.request_count.fetch_add(1, Ordering::SeqCst);
        debug!("Server received read({}) no. {}", size, request_number);
        self.pool
            .spawn(futures::lazy(move || {
                let mut vec = Vec::with_capacity(size as usize);
                for i in 0..size {
                    vec.push(((i % 2) << 8) as u8);
                }
                debug!("Server sending response no. {}", request_number);
                Ok(vec.into())
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

#[derive(Default)]
struct Stats {
    sum: Duration,
    count: u64,
    min: Option<Duration>,
    max: Option<Duration>,
}

/// Spawns a `reactor::Core` running forever on a new thread.
fn spawn_core() -> reactor::Remote {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut core = reactor::Core::new().unwrap();
        tx.send(core.handle().remote().clone()).unwrap();

        // Run forever
        core.run(futures::empty::<(), !>()).unwrap();
    });
    rx.recv().unwrap()
}

fn run_once(clients: Vec<FutureClient>,
            concurrency: u32)
            -> impl Future<Item = (), Error = ()> + 'static {
    let start = Instant::now();
    futures::stream::futures_unordered((0..concurrency as usize)
            .zip(clients.iter().enumerate().cycle())
            .map(|(iteration, (client_idx, client))| {
                let start = Instant::now();
                debug!("Client {} reading (iteration {})...", client_idx, iteration);
                client.read(CHUNK_SIZE)
                    .map(move |_| (client_idx, iteration, start))
            }))
        .map(|(client_idx, iteration, start)| {
            let elapsed = start.elapsed();
            debug!("Client {} received reply (iteration {}).",
                   client_idx,
                   iteration);
            elapsed
        })
        .map_err(|e| panic!(e))
        .fold(Stats::default(), move |mut stats, elapsed| {
            stats.sum += elapsed;
            stats.count += 1;
            stats.min = Some(cmp::min(stats.min.unwrap_or(elapsed), elapsed));
            stats.max = Some(cmp::max(stats.max.unwrap_or(elapsed), elapsed));
            Ok(stats)
        })
        .map(move |stats| {
            info!("{} requests => Mean={}µs, Min={}µs, Max={}µs, Total={}µs",
                  stats.count,
                  stats.sum.microseconds() as f64 / stats.count as f64,
                  stats.min.unwrap().microseconds(),
                  stats.max.unwrap().microseconds(),
                  start.elapsed().microseconds());
        })
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

    let mut reactor = reactor::Core::new().unwrap();
    let (handle, server) = Server::new()
        .listen("localhost:0".first_socket_addr(),
                &reactor.handle(),
                server::Options::default())
        .unwrap();
    reactor.handle().spawn(server);
    info!("Server listening on {}.", handle.addr());

    let clients = (0..num_clients)
        // Spin up a couple threads to drive the clients.
        .map(|i| (i, spawn_core()))
        .map(|(i, remote)| {
            info!("Client {} connecting...", i);
            FutureClient::connect(handle.addr(), client::Options::default().remote(remote))
                .map_err(|e| panic!(e))
        });

    let run = futures::collect(clients).and_then(|clients| run_once(clients, concurrency));

    info!("Starting...");

    reactor.run(run).unwrap();
}
