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

    use tarpc::protocol::ServeHandle;
    use self::test::Bencher;
    use std::sync::{Arc, Mutex};

    service! {
        rpc hello(s: String) -> String;
    }

    struct HelloServer;
    impl Service for HelloServer {
        fn hello(&mut self, mut ctx: Ctx, s: String) {
            ctx.hello(format!("Hello, {}!", s));
        }
    }

    // Prevents resource exhaustion when benching
    lazy_static! {
        static ref HANDLE: Arc<Mutex<ServeHandle>> = {
            let handle = HelloServer.spawn("localhost:0").unwrap();
            Arc::new(Mutex::new(handle))
        };
        static ref CLIENT: Arc<Mutex<Client>> = {
            let lock = HANDLE.lock().unwrap();
            let client = Client::spawn(lock.local_addr).unwrap();
            Arc::new(Mutex::new(client))
        };
    }

    #[bench]
    fn hello(bencher: &mut Bencher) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let _ = env_logger::init();
        let client = CLIENT.lock().unwrap();
        let concurrency = 100;
        let mut count = 0;
        let finished = Arc::new(AtomicUsize::new(0));
        let bob = "Bob".to_string();
        let current = ::std::thread::current();
        bencher.iter(|| {
            let fin = finished.clone();
            let cur = current.clone();
            client.hello(move |_reply| {
                _reply.unwrap();
                fin.fetch_add(1, Ordering::SeqCst);
                cur.unpark();
            }, &bob).unwrap();
            count += 1;
            if count % concurrency == 0 {
                while finished.load(Ordering::SeqCst) < count {
                    ::std::thread::park();
                }
            }
        });
    }
}
