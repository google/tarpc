// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(plugin)]
#![plugin(tarpc_plugins)]

#[macro_use]
extern crate log;
#[macro_use]
extern crate tarpc;
extern crate bincode;
extern crate env_logger;
extern crate futures;
extern crate tokio_core;

use bar::FutureServiceExt as BarExt;
use baz::FutureServiceExt as BazExt;
use std::sync::mpsc;
use std::thread;
use tarpc::{client, server};
use tarpc::client::sync::ClientExt;
use tarpc::util::{FirstSocketAddr, Never};
use tokio_core::reactor;

mod bar {
    service! {
        rpc bar(i: i32) -> i32;
    }
}

#[derive(Clone)]
struct Bar;
impl bar::FutureService for Bar {
    type BarFut = Result<i32, Never>;

    fn bar(&self, i: i32) -> Self::BarFut {
        Ok(i)
    }
}

mod baz {
    service! {
        rpc baz(s: String) -> String;
    }
}

#[derive(Clone)]
struct Baz;
impl baz::FutureService for Baz {
    type BazFut = Result<String, Never>;

    fn baz(&self, s: String) -> Self::BazFut {
        Ok(format!("Hello, {}!", s))
    }
}

macro_rules! pos {
    () => (concat!(file!(), ":", line!()))
}

fn main() {
    let _ = env_logger::init();
    let bar_client = {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reactor = reactor::Core::new().unwrap();
            let (handle, server) = Bar.listen("localhost:0".first_socket_addr(),
                        &reactor.handle(),
                        server::Options::default())
                .unwrap();
            tx.send(handle).unwrap();
            reactor.run(server).unwrap();
        });
        let handle = rx.recv().unwrap();
        bar::SyncClient::connect(handle.addr(), client::Options::default()).unwrap()
    };

    let baz_client = {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reactor = reactor::Core::new().unwrap();
            let (handle, server) = Baz.listen("localhost:0".first_socket_addr(),
                        &reactor.handle(),
                        server::Options::default())
                .unwrap();
            tx.send(handle).unwrap();
            reactor.run(server).unwrap();
        });
        let handle = rx.recv().unwrap();
        baz::SyncClient::connect(handle.addr(), client::Options::default()).unwrap()
    };


    info!("Result: {:?}", bar_client.bar(17));

    let total = 20;
    for i in 1..(total + 1) {
        if i % 2 == 0 {
            info!("Result 1: {:?}", bar_client.bar(i));
        } else {
            info!("Result 2: {:?}", baz_client.baz(i.to_string()));
        }
    }

    info!("Done.");
}
