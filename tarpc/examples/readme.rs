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

use futures::{
    future::{self, Ready},
    prelude::*,
};
use rpc::{
    client, context,
    server::{self, Handler, Server},
};
use std::io;

// This is the service definition. It looks a lot like a trait definition.
// It defines one RPC, hello, which takes one arg, name, and returns a String.
tarpc::service! {
    rpc hello(name: String) -> String;
}

// This is the type that implements the generated Service trait. It is the business logic
// and is used to start the server.
#[derive(Clone)]
struct HelloServer;

impl Service for HelloServer {
    // Each defined rpc generates two items in the trait, a fn that serves the RPC, and
    // an associated type representing the future output by the fn.

    type HelloFut = Ready<String>;

    fn hello(&self, _: context::Context, name: String) -> Self::HelloFut {
        future::ready(format!("Hello, {}!", name))
    }
}

async fn run() -> io::Result<()> {
    // bincode_transport is provided by the associated crate bincode-transport. It makes it easy
    // to start up a serde-powered bincode serialization strategy over TCP.
    let transport = bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?;
    let addr = transport.local_addr();

    // The server is configured with the defaults.
    let server = Server::new(server::Config::default())
        // Server can listen on any type that implements the Transport trait.
        .incoming(transport)
        // Close the stream after the client connects
        .take(1)
        // serve is generated by the tarpc::service! macro. It takes as input any type implementing
        // the generated Service trait.
        .respond_with(serve(HelloServer));

    tokio_executor::spawn(server.unit_error().boxed().compat());

    let transport = await!(bincode_transport::connect(&addr))?;

    // new_stub is generated by the tarpc::service! macro. Like Server, it takes a config and any
    // Transport as input, and returns a Client, also generated by the macro.
    // by the service mcro.
    let mut client = await!(new_stub(client::Config::default(), transport))?;

    // The client has an RPC method for each RPC defined in tarpc::service!. It takes the same args
    // as defined, with the addition of a Context, which is always the first arg. The Context
    // specifies a deadline and trace information which can be helpful in debugging requests.
    let hello = await!(client.hello(context::current(), "Stim".to_string()))?;

    println!("{}", hello);

    Ok(())
}

fn main() {
    tokio::run(
        run()
            .map_err(|e| eprintln!("Oh no: {}", e))
            .boxed()
            .compat(),
    );
}
