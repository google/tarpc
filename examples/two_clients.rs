#![feature(default_type_parameter_fallback)]
#[macro_use]
extern crate log;
#[macro_use]
extern crate tarpc;
extern crate serde;
extern crate bincode;
extern crate env_logger;
use tarpc::{Client, client, server};

mod bar {
    service! {
        rpc bar(i: i32) -> i32;
    }
}

struct Bar;
impl bar::AsyncService for Bar {
    fn bar(&mut self, ctx: bar::Ctx, i: i32) {
        ctx.bar(Ok(i)).unwrap();
    }
}

mod baz {
    service! {
        rpc baz(s: String) -> String;
    }
}

struct Baz;
impl baz::AsyncService for Baz {
    fn baz(&mut self, ctx: baz::Ctx, s: String) {
        ctx.baz(Ok(format!("Hello, {}!", s))).unwrap();
    }
}

macro_rules! pos {
    () => (concat!(file!(), ":", line!()))
}

use bar::AsyncService as BarService;
use baz::AsyncService as BazService;

fn main() {
    let _ = env_logger::init();
    let server_registry = server::Dispatcher::spawn().unwrap();
    let bar = Bar.register("localhost:0", &server_registry).unwrap();
    let baz = Baz.register("localhost:0", &server_registry).unwrap();

    info!("About to create Clients");
    let client_registry = client::Dispatcher::spawn().unwrap();
    let bar_client = bar::SyncClient::register(bar.local_addr(), &client_registry).unwrap();
    let baz_client = baz::SyncClient::register(baz.local_addr(), &client_registry).unwrap();

    info!("Result: {:?}", bar_client.bar(&17));

    let total = 20;
    for i in 1..(total+1) {
        if i % 2 == 0 {
            info!("Result 1: {:?}", bar_client.bar(&i));
        } else {
            info!("Result 2: {:?}", baz_client.baz(&i.to_string()));
        }
    }

    info!("Done.");
    client_registry.shutdown().expect(pos!());
    server_registry.shutdown().expect(pos!());
}
