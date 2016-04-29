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
use std::io::Cursor;
use std::thread;
use std::sync::mpsc::channel;
use tarpc::protocol::async::{self, Action, Dispatcher, SenderType};
use bincode::serde::{deserialize_from, serialize};
use bincode::SizeLimit;

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
    let mut event_loop = EventLoop::new().expect("D:");
    let client1 = async::Client::new(socket1);
    let client2 = async::Client::new(socket2);
    info!("About to run");
    let handle = event_loop.channel();
    thread::spawn(move || event_loop.run(&mut Dispatcher::new()).unwrap());

    let (tx, rx) = channel();
    handle.send(Action::Register(client1, tx)).unwrap();
    let token1 = rx.recv().unwrap();

    let (tx, rx) = channel();
    handle.send(Action::Register(client2, tx)).unwrap();
    let token2 = rx.recv().unwrap();

    let (tx1, rx1) = channel();
    let (tx2, rx2) = channel();
    for i in 0..20 {
        let req = __Request::bar((Foo { i: i },));
        if i % 2 == 0 {
            handle.send(Action::Rpc(token1,
                                    serialize(&req, SizeLimit::Infinite).unwrap(),
                                    SenderType::Mpsc(tx1.clone())))
                  .unwrap();
        } else {
            handle.send(Action::Rpc(token2,
                                    serialize(&req, SizeLimit::Infinite).unwrap(),
                                    SenderType::Mpsc(tx2.clone())))
                  .unwrap();
        }
    }
    for _ in 0..10 {
        info!("Result 1: {:?}",
              deserialize_from::<_, __Reply>(&mut Cursor::new(rx1.recv().unwrap().unwrap()),
                                             SizeLimit::Infinite));
        info!("Result 2: {:?}",
              deserialize_from::<_, __Reply>(&mut Cursor::new(rx2.recv().unwrap().unwrap()),
                                             SizeLimit::Infinite));
    }
    info!("Done.");
    handle.send(Action::Shutdown).unwrap();
}
