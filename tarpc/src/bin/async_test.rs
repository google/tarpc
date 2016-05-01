#![feature(custom_derive, plugin)]
#![plugin(serde_macros)]

#[macro_use]
extern crate log;
#[macro_use]
extern crate tarpc;
extern crate serde;
extern crate mio;
extern crate bincode;
extern crate env_logger;
use mio::*;
use mio::tcp::TcpStream;
use tarpc::protocol::async::{self, Dispatcher};

#[derive(Debug, Serialize, Deserialize)]
pub struct Foo {
    i: i32,
}

service! {
    rpc bar(foo: Foo) -> Foo;
}

struct Server;
impl Service for Server {
    fn bar(&self, foo: Foo) -> Foo {
        Foo { i: foo.i + 1 }
    }
}

fn main() {
    let _ = env_logger::init();
    let handle = Server.spawn("localhost:0").unwrap();
    info!("Sending message...");
    info!("About to create Client");
    let socket1 = TcpStream::connect(&handle.dialer().0).expect(":(");
    let socket2 = TcpStream::connect(&handle.dialer().0).expect(":(");
    let client1 = async::Client::new(socket1);
    let client2 = async::Client::new(socket2);

    info!("About to run");
    let request = __Request::bar((Foo { i: 17 },));
    let register = Dispatcher::spawn();
    let token1 = register.register(client1).unwrap();
    let future = register.rpc::<_, __Reply>(token1, &request);
    info!("Result: {:?}", future.unwrap().get());

    let token2 = register.register(client2).unwrap();

    let total = 20;
    let mut futures = Vec::with_capacity(total as usize);
    for i in 0..total {
        let req = __Request::bar((Foo { i: i },));
        if i % 2 == 0 {
            futures.push(register.rpc::<_, __Reply>(token1, &req).unwrap());
        } else {
            futures.push(register.rpc::<_, __Reply>(token2, &req).unwrap());
        }
    }
    for (i, fut) in futures.into_iter().enumerate() {
        if i % 2 == 0 {
            info!("Result 1: {:?}", fut.get().unwrap());
        } else {
            info!("Result 2: {:?}", fut.get().unwrap());
        }
    }
    info!("Done.");
    register.shutdown().unwrap();
    handle.shutdown();
}
