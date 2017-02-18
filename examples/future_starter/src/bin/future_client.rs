extern crate clap;
extern crate tarpc;
extern crate future_starter;
extern crate futures;
extern crate tokio_core;

use futures::Future;
use tarpc::client::Options;
use tarpc::client::future::ClientExt;
use tarpc::util::FirstSocketAddr;

fn main() {
    let matches = clap::App::new("hello future client")
        .arg(clap::Arg::with_name("server_address").required(true))
        .arg(clap::Arg::with_name("person_name").required(true))
        .get_matches();
    let addr = matches.value_of("server_address").unwrap().first_socket_addr();
    let person_name = matches.value_of("person_name").unwrap();
    let mut reactor = tokio_core::reactor::Core::new().unwrap();
    reactor.run(future_starter::FutureClient::connect(addr, Options::default())
            .map_err(tarpc::Error::from)
            .and_then(|client| client.hello(person_name.into()))
            .map(|resp| println!("{}", resp)))
        .unwrap();
}
