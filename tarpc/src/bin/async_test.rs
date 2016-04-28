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
use std::thread;
use std::sync::mpsc::channel;
use tarpc::protocol::async::{self, SenderType};

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
    let socket = TcpStream::connect(&handle.dialer().0).expect(":(");
    let mut event_loop = EventLoop::new().expect("D:");
    let handle = event_loop.channel();
    let mut client: async::Client<__Request, __Reply> = async::Client::new(socket);
    info!("About to run");
    thread::spawn(move || event_loop.run(&mut client).unwrap());
    let (tx, rx) = channel();
    let req = __Request::bar((Foo { i: 1 },));
    handle.send((req, SenderType::Mpsc(tx))).unwrap();
    info!("Result: {:?}", rx.recv().unwrap());
    info!("Done.");
}
