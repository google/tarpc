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
use tarpc::protocol::client::Dispatcher;
use std::net::ToSocketAddrs;

service! {
    rpc bar(i: i32) -> i32;
}

struct Server;
impl BlockingService for Server {
    fn bar(&self, i: i32) -> i32 {
        i + 1
    }
}

impl Service for Server {
    fn bar(&mut self, ctx: RequestContext, i: i32) {
        ctx.bar(i);
    }
}

macro_rules! pos {
    () => (concat!(file!(), ":", line!()))
}

fn main() {
    let _ = env_logger::init();
    let addr = "127.0.0.1:58765".to_socket_addrs().expect(pos!()).next().expect(pos!());
    let handle = Service::spawn(Server, addr).unwrap();

    info!("About to create Client");
    let socket1 = TcpStream::connect(&addr).expect(":(");
    let socket2 = TcpStream::connect(&addr).expect(":(");

    info!("About to run");
    let i = 17;
    let i = (&i,);
    let request = __ClientSideRequest::bar(&i);
    let register = Dispatcher::spawn();
    let client1 = register.register(socket1).expect(pos!());
    let future = client1.rpc::<_, __Reply>(&request);
    info!("Result: {:?}", future.unwrap().get().expect(pos!()));

    let client2 = register.register(socket2).expect(pos!());

    let total = 20;
    let mut futures = Vec::with_capacity(total as usize);
    for i in 1..(total+1) {
        let req = (&i,);
        let req = __ClientSideRequest::bar(&req);
        if i % 2 == 0 {
            futures.push(client1.rpc::<_, __Reply>(&req).expect(pos!()));
        } else {
            futures.push(client2.rpc::<_, __Reply>(&req).expect(pos!()));
        }
    }
    for (i, fut) in futures.into_iter().enumerate() {
        if i % 2 == 0 {
            info!("Result 1: {:?}", fut.get().expect(pos!()));
        } else {
            info!("Result 2: {:?}", fut.get().expect(pos!()));
        }
    }
    info!("Done.");
    register.shutdown().expect(pos!());
    handle.shutdown().expect(pos!());
}
