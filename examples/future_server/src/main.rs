extern crate clap;
extern crate hello_api;
extern crate tarpc;
extern crate tokio_core;

use hello_api::{FutureServiceExt, FutureService};
use tarpc::future::server::Options;
use tarpc::util::{Never, FirstSocketAddr};

#[derive(Clone)]
pub struct HelloServer;

impl FutureService for HelloServer {
    type HelloFut = Result<String, Never>;

    fn hello(&self, name: String) -> Self::HelloFut {
        Ok(format!("Hello, {}!", name))
    }
}

fn main() {
    let matches = clap::App::new("hello sync client")
        .arg(clap::Arg::with_name("port").required(true))
        .get_matches();
    let port: u16 = matches.value_of("port").unwrap().parse().unwrap();
    let mut reactor = tokio_core::reactor::Core::new().unwrap();
    let (handle, server) = HelloServer.listen(format!("localhost:{}", port).first_socket_addr(),
                &reactor.handle(),
                Options::default())
        .unwrap();
    println!("Listening on {}", handle.addr());
    reactor.run(server).unwrap();
}
