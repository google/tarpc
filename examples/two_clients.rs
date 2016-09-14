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

use bar::FutureServiceExt as BarExt;
use baz::FutureServiceExt as BazExt;
use futures::Future;
use tarpc::util::Never;
use tarpc::sync::Connect;

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
    let bar = Bar.listen("localhost:0").wait().unwrap();
    let baz = Baz.listen("localhost:0").wait().unwrap();
    let bar_client = bar::SyncClient::connect(bar.local_addr()).unwrap();
    let baz_client = baz::SyncClient::connect(baz.local_addr()).unwrap();

    info!("Result: {:?}", bar_client.bar(&17));

    let total = 20;
    for i in 1..(total + 1) {
        if i % 2 == 0 {
            info!("Result 1: {:?}", bar_client.bar(&i));
        } else {
            info!("Result 2: {:?}", baz_client.baz(&i.to_string()));
        }
    }

    info!("Done.");
}
