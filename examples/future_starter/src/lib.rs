#![feature(conservative_impl_trait, plugin)]
#![plugin(tarpc_plugins)]

#[macro_use]
extern crate tarpc;

use tarpc::util::Never;

service! {
    rpc hello(name: String) -> String;
}

#[derive(Clone)]
pub struct HelloServer;

impl FutureService for HelloServer {
    type HelloFut = Result<String, Never>;

    fn hello(&self, name: String) -> Self::HelloFut {
        Ok(format!("Hello, {}!", name))
    }
}
