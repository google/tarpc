#![feature(default_type_parameter_fallback)]
#[macro_use]
extern crate tarpc;
use tarpc::Client;

mod hello_service {
    service! {
        rpc hello(name: String) -> String;
    }
}
use hello_service::{ServiceExt, SyncService as HelloService};

#[derive(Clone)]
struct HelloServer;
impl HelloService for HelloServer {
    fn hello(&self, name: String) -> tarpc::RpcResult<String> {
        Ok(format!("Hello, {}!", name))
    }
}

fn main() {
    let addr = "localhost:10000";
    HelloServer.listen(addr).unwrap();
    let client = hello_service::SyncClient::connect(addr).unwrap();
    assert_eq!("Hello, Mom!", client.hello(&"Mom".to_string()).unwrap());
}
