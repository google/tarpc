#[macro_use]
extern crate log;
#[macro_use]
extern crate tarpc;
extern crate serde;
extern crate mio;
extern crate bincode;
extern crate env_logger;
use mio::*;
use tarpc::protocol::{client, server};

mod bar {
    service! {
        rpc bar(i: i32) -> i32;
    }
}

struct Bar;
impl bar::Service for Bar {
    fn bar(&mut self, ctx: bar::RequestContext, i: i32) {
        ctx.bar(i);
    }
}

mod baz {
    service! {
        rpc baz(s: String) -> String;
    }
}

struct Baz;
impl baz::Service for Baz {
    fn baz(&mut self, ctx: baz::RequestContext, s: String) {
        ctx.baz(format!("Hello, {}!", s));
    }
}

macro_rules! pos {
    () => (concat!(file!(), ":", line!()))
}

use bar::Service as BarService;
use baz::Service as BazService;

fn main() {
    let _ = env_logger::init();
    let server_registry = server::Dispatcher::spawn();
    let bar = Bar.register("localhost:0", &server_registry).unwrap();
    let baz = Baz.register("localhost:0", &server_registry).unwrap();

    info!("About to create Clients");
    let client_registry = client::Dispatcher::spawn();
    let bar_client = bar::Client::register(bar.local_addr, &client_registry).unwrap();
    let baz_client = baz::Client::register(baz.local_addr, &client_registry).unwrap();

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
