// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(plugin, futures_api, pin, arbitrary_self_types, await_macro, async_await, existential_type)]
#![plugin(tarpc_plugins)]

#[macro_use]
extern crate tarpc;

use futures::{prelude::*, future::{self, Ready}};
use tokio::net::{TcpListener, TcpStream};
use rpc::{
    client::{self},
    server::{self, Server},
};
use std::io;
use futures::compat::{Future01CompatExt, Stream01CompatExt, TokioDefaultSpawner};

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

    let listener = TcpListener::bind(&"0.0.0.0:0".parse().unwrap())?;
    let addr = listener.local_addr()?;
    let transport = listener.incoming().compat().take(1).map_ok(bincode_transport::new);
    let server = Server::new(server::Config::default())
        .incoming(transport)
        .respond_with(serve(HelloServer));

    let requests = async move {
        let stream = await!(TcpStream::connect(&addr).compat())?;
        let transport = bincode_transport::new(stream);
        let mut client = await!(newStub(client::Config::default(), transport));
        let hello_max = await!(client.hello(client::Context::current(), "Max".to_string()))?;
        let hello_adam = await!(client.hello(client::Context::current(), "Adam".to_string()))?;
        println!("{} {}", hello_max, hello_adam);
        Ok::<_, io::Error>(())
    };

    tokio::run(server.join(requests)
            .map(|(_, _)| {})
            .boxed()
            .unit_error()
            .compat(TokioDefaultSpawner),
    );
    Ok(())
}
