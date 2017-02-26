// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(plugin)]
#![plugin(tarpc_plugins)]

extern crate futures;
#[macro_use]
extern crate tarpc;
extern crate tokio_core;

use futures::Future;
use tarpc::future::{client, server};
use tarpc::future::client::ClientExt;
use tarpc::util::{FirstSocketAddr, Never};
use tokio_core::reactor;

service! {
    rpc hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl FutureService for HelloServer {
    type HelloFut = Result<String, Never>;

    fn hello(&self, name: String) -> Self::HelloFut {
        Ok(format!("Hello, {}!", name))
    }
}

fn main() {
    let mut reactor = reactor::Core::new().unwrap();
    let (handle, server) = HelloServer.listen("localhost:10000".first_socket_addr(),
                &reactor.handle(),
                server::Options::default())
        .unwrap();
    reactor.handle().spawn(server);

    let options = client::Options::default().handle(reactor.handle());
    reactor.run(FutureClient::connect(handle.addr(), options)
            .map_err(tarpc::Error::from)
            .and_then(|client| client.hello("Mom".to_string()))
            .map(|resp| println!("{}", resp)))
        .unwrap();
}
