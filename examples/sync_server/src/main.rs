extern crate hello_api;
extern crate tarpc;
extern crate clap;

use hello_api::SyncServiceExt;
use tarpc::sync::server::Options;
use tarpc::util::Never;

#[derive(Clone)]
pub struct HelloServer;

impl hello_api::SyncService for HelloServer {
    fn hello(&self, name: String) -> Result<String, Never> {
        Ok(format!("Hey {}!", name))
    }
}

fn main() {
    let matches = clap::App::new("hello sync client")
        .arg(clap::Arg::with_name("port").required(true))
        .get_matches();
    let port: u16 = matches.value_of("port").unwrap().parse().unwrap();
    let handle = HelloServer.listen(format!("localhost:{}", port), Options::default()).unwrap();
    println!("Listening on {}", handle.addr());
    handle.run();
}
