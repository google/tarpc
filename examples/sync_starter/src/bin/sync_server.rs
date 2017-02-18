extern crate sync_starter;
extern crate tarpc;

use sync_starter::SyncServiceExt;
use tarpc::server::Options;

fn main() {
    let mut handle = sync_starter::HelloServer.listen("localhost:0", Options::default()).unwrap();
    println!("Listening on {}", handle.addr());
    handle.run();
}
