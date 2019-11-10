// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use clap::{App, Arg};
use futures::{
    future::{self, Ready},
    prelude::*,
};
use service::World;
use std::{
    io,
    net::{IpAddr, SocketAddr},
};
use tarpc::{
    context,
    server::{self, Channel, Handler},
};

// This is the type that implements the generated World trait. It is the business logic
// and is used to start the server.
#[derive(Clone)]
struct HelloServer(SocketAddr);

impl World for HelloServer {
    // Each defined rpc generates two items in the trait, a fn that serves the RPC, and
    // an associated type representing the future output by the fn.

    type HelloFut = Ready<String>;

    fn hello(self, _: context::Context, name: String) -> Self::HelloFut {
        future::ready(format!(
            "Hello, {}! You are connected from {:?}.",
            name, self.0
        ))
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    env_logger::init();

    let flags = App::new("Hello Server")
        .version("0.1")
        .author("Tim <tikue@google.com>")
        .about("Say hello!")
        .arg(
            Arg::with_name("port")
                .short("p")
                .long("port")
                .value_name("NUMBER")
                .help("Sets the port number to listen on")
                .required(true)
                .takes_value(true),
        )
        .get_matches();

    let port = flags.value_of("port").unwrap();
    let port = port
        .parse()
        .unwrap_or_else(|e| panic!(r#"--port value "{}" invalid: {}"#, port, e));

    let server_addr = (IpAddr::from([0, 0, 0, 0]), port).into();

    // tarpc_bincode_transport is provided by the associated crate tarpc-bincode-transport.
    // It makes it easy to start up a serde-powered bincode serialization strategy over TCP.
    let mut incoming = tarpc_bincode_transport::listen(&server_addr)?
        // Ignore accept errors.
        .filter_map(|r| future::ready(r.ok()))
        .map(|t| {
            let mut config = server::Config::default();
            config.pending_response_buffer = 256;
            server::BaseChannel::new(config, t)
        })
        // limit connections to 1 per IP
        // disabled to allow for load testing
        // .max_channels_per_key(1, |t| t.as_ref().peer_addr().unwrap().ip())
        .max_concurrent_requests_per_channel(5);

    while let Some(channel) = incoming.next().await {
        tokio::spawn(async move {
            let server = HelloServer(channel.as_ref().as_ref().as_ref().peer_addr().unwrap());
            channel.respond_with(server.serve()).execute().await;
        });
    }

    Ok(())
}
