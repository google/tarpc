#![feature(default_type_parameter_fallback)]
#[macro_use]
extern crate log;
#[macro_use]
extern crate tarpc;
extern crate serde;
extern crate bincode;
extern crate env_logger;

use bar::AsyncServiceExt as BarExt;
use baz::AsyncServiceExt as BazExt;
use tarpc::Client;

mod bar {
    service! {
        rpc bar(i: i32) -> i32;
    }
}

struct Bar;
impl bar::AsyncService for Bar {
    fn bar(&mut self, ctx: tarpc::Ctx, i: i32) {
        use bar::Ctx;
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
    fn baz(&mut self, ctx: tarpc::Ctx, s: String) {
        use baz::Ctx;
        ctx.baz(Ok(format!("Hello, {}!", s))).unwrap();
    }
}

macro_rules! pos {
    () => (concat!(file!(), ":", line!()))
}

fn main() {
    let _ = env_logger::init();
    let bar = Bar.listen("localhost:0").unwrap();
    let baz = Baz.listen("localhost:0").unwrap();
    let bar_client = bar::SyncClient::connect(bar.local_addr()).unwrap();
    let baz_client = baz::SyncClient::connect(baz.local_addr()).unwrap();

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
}
