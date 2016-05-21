#![feature(default_type_parameter_fallback)]
#[macro_use]
extern crate tarpc;

use tarpc::Client;

service! {
    rpc hello(name: String) -> String;
}

#[derive(Clone, Copy)]
struct HelloServer;

impl SyncService for HelloServer {
    fn hello(&self, name: String) -> tarpc::RpcResult<String> {
        Ok(format!("Hello, {}!", name))
    }
}

fn main() {
    let addr = "localhost:10000";
    HelloServer.listen(addr).unwrap();
    let client = SyncClient::connect(addr).unwrap();
    assert_eq!("Hello, Mom!", client.hello(&"Mom".to_string()).unwrap());
}
