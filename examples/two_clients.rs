// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(conservative_impl_trait, plugin)]
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
    type BarFut = futures::Finished<i32, Never>;

    fn bar(&self, i: i32) -> Self::BarFut {
        futures::finished(i)
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
    type BazFut = futures::Finished<String, Never>;

    fn baz(&self, s: String) -> Self::BazFut {
        futures::finished(format!("Hello, {}!", s))
    }
}

macro_rules! pos {
    () => (concat!(file!(), ":", line!()))
}

fn main() {
    let _ = env_logger::init();
    let mut bar_client = {
        let reactor = reactor::Core::new().unwrap();
        let addr = Bar.listen("localhost:0".first_socket_addr(),
                    &reactor.handle(),
                    server::Options::default())
            .unwrap();
        // TODO: Need to set up each client with its own reactor. Should it be shareable across
        // multiple clients? e.g. Rc<RefCell<Core>> or something similar?
        bar::SyncClient::connect(addr, client::Options::default()).unwrap()
    };

    let mut baz_client = {
        // Need to set up each client with its own reactor.
        let reactor = reactor::Core::new().unwrap();
        let addr = Baz.listen("localhost:0".first_socket_addr(),
                    &reactor.handle(),
                    server::Options::default())
            .unwrap();
        baz::SyncClient::connect(addr, client::Options::default()).unwrap()
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
