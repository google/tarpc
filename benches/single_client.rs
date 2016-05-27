// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(default_type_parameter_fallback, try_from)]
#![feature(default_type_parameter_fallback, test, try_from)]

#[cfg(test)]
#[macro_use]
extern crate lazy_static;

#[cfg(test)]
#[macro_use]
extern crate tarpc;

#[macro_use]
extern crate log;

#[cfg(test)]
#[allow(dead_code)] // generated Client isn't used in this benchmark
mod benchmark {
    extern crate env_logger;
    extern crate test;

    use tarpc::{self, Client, Ctx, ServeHandle};
    use self::test::Bencher;
    use std::sync::{Arc, Mutex};

    service! {
        rpc hello(s: String) -> String;
    }

    struct HelloServer;
    impl AsyncService for HelloServer {
        fn hello(&self, ctx: Ctx<String>, s: String) {
            ctx.reply(Ok(s)).unwrap();
        }
    }

    // Prevents resource exhaustion when benching
    lazy_static! {
        static ref HANDLES: Arc<Mutex<Vec<ServeHandle>>> = {
            let handles = (0..2).map(|_| {
                let registry = tarpc::server::Dispatcher::spawn().unwrap();
                HelloServer.register("localhost:0", tarpc::server::Config::registry(registry)).unwrap()
            }).collect();
            Arc::new(Mutex::new(handles))
        };
        static ref CLIENTS: Arc<Mutex<Vec<AsyncClient>>> = {
            let lock = HANDLES.lock().unwrap();
            let clients = (0..35).map(|i| Client::connect(lock[i % lock.len()].local_addr()).unwrap()).collect();
            Arc::new(Mutex::new(clients))
        };
    }

    #[bench]
    fn hello(bencher: &mut Bencher) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let _ = env_logger::init();
        let clients = CLIENTS.lock().unwrap();
        let mut clients = clients.iter().cycle();
        let concurrency = 1200;
        let mut count = 0;
        let finished = Arc::new(AtomicUsize::new(0));
        let bob = "Bob".to_string();
        let current = ::std::thread::current();
        bencher.iter(|| {
            let fin = finished.clone();
            let cur = current.clone();
            clients.next().unwrap().hello(move |_reply| {
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
