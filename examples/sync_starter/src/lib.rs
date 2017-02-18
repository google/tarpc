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

impl SyncService for HelloServer {
    fn hello(&self, name: String) -> Result<String, Never> {
        Ok(format!("Hey {}!", name))
    }
}
