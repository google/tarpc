// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![cfg_attr(test, feature(test))]

#[cfg(test)]
#[macro_use]
extern crate lazy_static;

#[cfg(test)]
#[macro_use]
extern crate tarpc;

#[cfg(test)]
#[allow(dead_code)] // generated Client isn't used in this benchmark
mod benchmark {
    extern crate env_logger;
    extern crate test;

    use tarpc::ServeHandle;
    use self::test::Bencher;
    use std::sync::{Arc, Mutex};

    service! {
        rpc hello(s: String) -> String;
    }

    struct HelloServer;
    impl Service for HelloServer {
        fn hello(&self, s: String) -> String {
            format!("Hello, {}!", s)
        }
    }

    // Prevents resource exhaustion when benching
    lazy_static! {
        static ref HANDLE: Arc<Mutex<ServeHandle>> = {
            let handle = serve("localhost:0", HelloServer, None).unwrap();
            Arc::new(Mutex::new(handle))
        };
        static ref CLIENT: Arc<Mutex<AsyncClient>> = {
            let addr = HANDLE.lock().unwrap().local_addr().clone();
            let client = AsyncClient::new(addr, None).unwrap();
            Arc::new(Mutex::new(client))
        };
    }

    #[bench]
    fn hello(bencher: &mut Bencher) {
        let _ = env_logger::init();
        let client = CLIENT.lock().unwrap();
        let concurrency = 100;
        let mut futures = Vec::with_capacity(concurrency);
        let mut count = 0;
        bencher.iter(|| {
            futures.push(client.hello("Bob".into()));
            count += 1;
            if count % concurrency == 0 {
                // We can't block on each rpc call, otherwise we'd be
                // benchmarking latency instead of throughput. It's also
                // not ideal to call more than one rpc per iteration, because
                // it makes the output of the bencher harder to parse (you have
                // to mentally divide the number by `concurrency` to get
                // the ns / iter for one rpc
                for f in futures.drain(..) {
                    f.get().unwrap();
                }
            }
        });
    }
}
