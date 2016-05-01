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
use tarpc::protocol::{Dispatcher, Packet};

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

fn main() {
    let _ = env_logger::init();
    let handle = Server.spawn("localhost:0").unwrap();
    info!("Sending message...");
    info!("About to create Client");
    let socket1 = TcpStream::connect(&handle.dialer().0).expect(":(");
    let socket2 = TcpStream::connect(&handle.dialer().0).expect(":(");

    info!("About to run");
    let request = __Request::bar((Packet { rpc_id: 0, message: 17 },));
    let register = Dispatcher::spawn();
    let client1 = register.register::<__Request, __Reply>(socket1).unwrap();
    let future = client1.rpc(&request);
    info!("Result: {:?}", future.unwrap().get());

    let client2 = register.register::<__Request, __Reply>(socket2).unwrap();

    let total = 20;
    let mut futures = Vec::with_capacity(total as usize);
    for i in 0..total {
        let req = __Request::bar((Packet { rpc_id: 0, message: i },));
        if i % 2 == 0 {
            futures.push(client1.rpc(&req).unwrap());
        } else {
            futures.push(client2.rpc(&req).unwrap());
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
