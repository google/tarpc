extern crate clap;
extern crate tarpc;
extern crate sync_starter;

use tarpc::client::Options;
use tarpc::client::sync::ClientExt;

fn main() {
    let matches = clap::App::new("hello sync client")
        .arg(clap::Arg::with_name("server_address").required(true))
        .arg(clap::Arg::with_name("person_name").required(true))
        .get_matches();
    let addr = matches.value_of("server_address").unwrap();
    let person_name = matches.value_of("person_name").unwrap();
    let mut client = sync_starter::SyncClient::connect(addr, Options::default()).unwrap();
    println!("{}", client.hello(person_name.into()).unwrap());
}