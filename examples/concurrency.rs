// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![plugin(serde_macros)]
#![feature(custom_derive, plugin, default_type_parameter_fallback, test, try_from, inclusive_range_syntax)]

extern crate chrono;
extern crate env_logger;
#[macro_use]
extern crate log;
extern crate mio;
extern crate serde;
#[macro_use]
extern crate tarpc;
extern crate test;

use serde::{Serialize, Deserialize};
use std::ops::Add;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tarpc::{Client, RpcResult};

service! {
    rpc read(size: u32, sent_at: SerializableInstant) -> (Vec<u8>, SerializableInstant);
}

struct Server;

impl SyncService for Server {
    fn read(&self, size: u32, sent_at: SerializableInstant) -> RpcResult<(Vec<u8>, SerializableInstant)>  {
        return Ok((gen_vec(size as usize), sent_at));

        fn gen_vec(size: usize) -> Vec<u8> {
            let mut vec: Vec<u8> = Vec::with_capacity(size);
            for i in 0..size {
                vec.push((i % 1 << 8) as u8);
            }
            vec
        }
    }
}

fn run_once(clients: &[FutureClient], concurrency: u32, print: bool) {
    let _ = env_logger::init();

    let start = Instant::now();
    let futures: Vec<_> = 
        clients.iter()
               .cycle()
               .take(concurrency as usize)
               .map(|client| client.read(&CHUNK_SIZE, &SystemTime::now().serializable()))
               .collect();
    let sum_latencies = futures.into_iter()
        .map(|future| {
            let start = future.get().unwrap().1.to_duration();
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap() - start
        })
        .fold(Duration::new(0, 0), Duration::add);
    let total_time = start.elapsed();
    if print {
        println!("Mean time per request: {} µs, Total time for {} requests: {} µs",
             chrono::Duration::from_std(sum_latencies / concurrency).unwrap().num_microseconds().unwrap(),
             concurrency,
             chrono::Duration::from_std(total_time).unwrap().num_microseconds().unwrap());
    }
}

trait Serializable {
    type T: Serialize + Deserialize;
    
    fn serializable(self) -> Self::T;
}

impl Serializable for SystemTime {
    type T = SerializableInstant;

    fn serializable(self) -> SerializableInstant {
        let epoch_distance = self.duration_since(UNIX_EPOCH).unwrap();
        SerializableInstant {
            secs_from_epoch: epoch_distance.as_secs(),
            subsec_nanos_from_epoch: epoch_distance.subsec_nanos(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
pub struct SerializableInstant {
    secs_from_epoch: u64,
    subsec_nanos_from_epoch: u32,
}

impl SerializableInstant {
    fn to_duration(self) -> Duration {
        Duration::new(self.secs_from_epoch, self.subsec_nanos_from_epoch)
    }
}

const CHUNK_SIZE: u32 = 1 << 18;
const MAX_CONCURRENCY: u32 = 1000;

fn main() {
    let server = Server.listen("localhost:0").unwrap();
    let clients: Vec<_> = (1...5).map(|_| {
        FutureClient::connect(server.local_addr().unwrap()).unwrap()
    }).collect();

    run_once(&clients, MAX_CONCURRENCY, false);
    for concurrency in 1...MAX_CONCURRENCY {
        run_once(&clients, concurrency, true);
    }
}
