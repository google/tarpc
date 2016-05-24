// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

extern crate env_logger;
#[macro_use]
extern crate log;
extern crate tarpc;

use std::thread;
use std::time::Duration;
use tarpc::cached_pool::{CachedPool, Config};

fn main() {
    let _ = env_logger::init();

    let pools = &[CachedPool::new(Config::max_idle(Duration::from_secs(5))),
                  CachedPool::new(Config::max_idle(Duration::from_millis(500)))];
    for _ in 0..15 {
        for pool in pools {
            pool.execute(move || {
                    thread::sleep(Duration::from_secs(5));
                })
                .unwrap();
        }
        info!("{:?}",
              pools.iter().map(CachedPool::debug).collect::<Vec<_>>());
        thread::sleep(Duration::from_millis(500));
    }
    for _ in 0..7 {
        for pool in pools {
            pool.execute(move || {
                    thread::sleep(Duration::from_secs(5));
                })
                .unwrap();
        }
        info!("{:?}",
              pools.iter().map(CachedPool::debug).collect::<Vec<_>>());
        thread::sleep(Duration::from_secs(1));
    }
    info!("Almost done...");
    thread::sleep(Duration::from_millis(5500));
    info!("Done.");
}

