extern crate future_starter;
extern crate tarpc;
extern crate tokio_core;

use future_starter::FutureServiceExt;
use tarpc::future::server::Options;
use tarpc::util::FirstSocketAddr;

fn main() {
    let mut reactor = tokio_core::reactor::Core::new().unwrap();
    let (handle, server) = future_starter::HelloServer.listen("localhost:0".first_socket_addr(),
                &reactor.handle(),
                Options::default())
        .unwrap();
    println!("Listening on {}", handle.addr());
    reactor.run(server).unwrap();
}
