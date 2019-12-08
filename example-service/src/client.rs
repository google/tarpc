// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use clap::{App, Arg};
use std::{io, net::SocketAddr};
use tarpc::{client, context};
use tokio_serde::formats::Json;

#[tokio::main]
async fn main() -> io::Result<()> {
    let flags = App::new("Hello Client")
        .version("0.1")
        .author("Tim <tikue@google.com>")
        .about("Say hello!")
        .arg(
            Arg::with_name("server_addr")
                .long("server_addr")
                .value_name("ADDRESS")
                .help("Sets the server address to connect to.")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("name")
                .short("n")
                .long("name")
                .value_name("STRING")
                .help("Sets the name to say hello to.")
                .required(true)
                .takes_value(true),
        )
        .get_matches();

    let server_addr = flags.value_of("server_addr").unwrap();
    let server_addr = server_addr
        .parse::<SocketAddr>()
        .unwrap_or_else(|e| panic!(r#"--server_addr value "{}" invalid: {}"#, server_addr, e));

    let name = flags.value_of("name").unwrap().into();

    let transport = tarpc::serde_transport::tcp::connect(server_addr, Json::default()).await?;

    // WorldClient is generated by the service attribute. It has a constructor `new` that takes a
    // config and any Transport as input.
    let mut client = service::WorldClient::new(client::Config::default(), transport).spawn()?;

    // The client has an RPC method for each RPC defined in the annotated trait. It takes the same
    // args as defined, with the addition of a Context, which is always the first arg. The Context
    // specifies a deadline and trace information which can be helpful in debugging requests.
    let hello = client.hello(context::current(), name).await?;

    println!("{}", hello);

    Ok(())
}
