#[macro_use]
extern crate log;
#[macro_use]
extern crate tarpc;
extern crate serde;
extern crate mio;
extern crate bincode;
extern crate env_logger;
use mio::*;
use mio::tcp::{TcpListener, TcpStream};
use tarpc::protocol::{self, Dispatcher, Packet};
use tarpc::protocol::async::server;
use std::net::ToSocketAddrs;

service! {
    rpc bar(packet: Packet<i32>) -> Packet<i32>;
}

struct Server;
impl Service for Server {
    fn bar(&self, packet: Packet<i32>) -> Packet<i32> {
        Packet {
            rpc_id: packet.rpc_id,
            message: packet.message + 1,
        }
    }
}

impl server::Service for Server {
    fn handle(&mut self, token: Token, packet: protocol::client::Packet, event_loop: &mut EventLoop<server::Dispatcher>) {
        event_loop.channel().send(server::Action::Reply(token, packet)).unwrap();
    }
}

macro_rules! pos {
    () => (concat!(file!(), ":", line!()))
}

fn main() {
    let _ = env_logger::init();
    let addr = "127.0.0.1:58765".to_socket_addrs().expect(pos!()).next().expect(pos!());
    let socket = TcpListener::bind(&addr).expect(pos!());
    let server_register = server::Dispatcher::spawn();
    server_register.register(server::Server::new(socket, Box::new(Server))).expect(pos!());

    info!("About to create Client");
    let socket1 = TcpStream::connect(&addr).expect(":(");
    let socket2 = TcpStream::connect(&addr).expect(":(");

    info!("About to run");
    let packet = Packet { rpc_id: 0, message: 17 };
    let packet = (&packet,);
    let request = __ClientSideRequest::bar(&packet);
    let register = Dispatcher::spawn();
    let client1 = register.register(socket1).expect(pos!());
    let future = client1.rpc::<_, __Reply>(&request);
    info!("Result: {:?}", future.unwrap().get().expect(pos!()));

    let client2 = register.register(socket2).expect(pos!());

    let total = 20;
    let mut futures = Vec::with_capacity(total as usize);
    for i in 1..(total+1) {
        let packet = (&Packet { rpc_id: 0, message: i },);
        let req = __ClientSideRequest::bar(&packet);
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
    server_register.shutdown().expect(pos!());
}
