// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(
    plugin,
    futures_api,
    pin,
    arbitrary_self_types,
    await_macro,
    async_await,
    existential_type
)]
#![plugin(tarpc_plugins)]

#[macro_use]
extern crate tarpc;

use futures::compat::TokioDefaultSpawner;
use futures::{
    future::{self, Ready},
    prelude::*,
};
use rpc::{
    client,
    server::{self, Server},
};
use std::io;

service! {
    rpc hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl Service for HelloServer {
    type HelloFut = Ready<String>;

    fn hello(&self, _: &server::Context, name: String) -> Self::HelloFut {
        future::ready(format!("Hello, {}!", name))
    }
}

fn main() -> io::Result<()> {
    env_logger::init();

    let transport = bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?;
    let addr = transport.local_addr();
    let server = Server::new(server::Config::default())
        .incoming(transport)
        .respond_with(serve(HelloServer));

    let requests = async move {
        let transport = await!(bincode_transport::connect(&addr))?;
        let mut client = await!(newStub(client::Config::default(), transport));
        let hello_max = await!(client.hello(client::Context::current(), "Max".to_string()))?;
        let hello_adam = await!(client.hello(client::Context::current(), "Adam".to_string()))?;
        println!("{} {}", hello_max, hello_adam);
        Ok::<_, io::Error>(())
    };

    tokio::run(
        server
            .join(requests)
            .map(|(_, _)| {})
            .boxed()
            .unit_error()
            .compat(TokioDefaultSpawner),
    );
    Ok(())
}
