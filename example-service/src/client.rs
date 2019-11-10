// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use clap::{App, Arg};
use futures::stream::StreamExt;
use std::{io, net::SocketAddr, time::Instant};
use tarpc::{client, context};

const NAME: &'static str = "foo";
const ROUNDS: usize = 1000;

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
            Arg::with_name("num_connections")
                .long("num_connections")
                .required(true)
                .takes_value(true),
        )
        .get_matches();

    let server_addr = flags.value_of("server_addr").unwrap();
    let server_addr = server_addr
        .parse::<SocketAddr>()
        .unwrap_or_else(|e| panic!(r#"--server_addr value "{}" invalid: {}"#, server_addr, e));

    let num_connections: usize = flags
        .value_of("num_connections")
        .unwrap()
        .parse()
        .expect("num_connections needs an argument");

    let (senders, receivers): (
        Vec<tokio::sync::mpsc::Sender<Instant>>,
        Vec<tokio::sync::mpsc::Receiver<Instant>>,
    ) = (0..num_connections)
        .map(|_| tokio::sync::mpsc::channel(ROUNDS + 1))
        .unzip();

    for mut sender in senders.into_iter() {
        tokio::spawn(async move {
            sender
                .send(Instant::now())
                .await
                .expect("failed to send instant");
            let transport = tarpc_bincode_transport::connect(&server_addr)
                .await
                .expect("failed to connect");
            sender
                .send(Instant::now())
                .await
                .expect("failed to send instant");

            let mut client = service::WorldClient::new(client::Config::default(), transport)
                .spawn()
                .expect("failed to spawn client");

            for _ in 0..ROUNDS {
                client
                    .hello(context::current(), NAME.into())
                    .await
                    .expect("failed to run hello");
                sender
                    .send(Instant::now())
                    .await
                    .expect("failed to send instant");
            }
        });
    }

    let mut latencies = Vec::new();
    let mut instants = Vec::new();
    for r in receivers.into_iter() {
        let ivec: Vec<_> = r.collect().await;
        for i in ivec.iter() {
            instants.push(*i);
        }
        for (i1, i2) in ivec.iter().zip(&ivec[1..]) {
            let dur = i2.duration_since(*i1);
            latencies.push(dur);
        }
    }

    latencies.sort_unstable();
    instants.sort_unstable();

    let num_instants = instants.len();

    println!("minimum latency was {} micros", latencies[0].as_micros());
    println!(
        "maximum latency was {} micros",
        latencies
            .pop()
            .expect("failed to record a single latency")
            .as_micros()
    );
    println!(
        "median latency was {} micros",
        latencies[num_instants / 2].as_micros()
    );

    let total_seconds = instants
        .pop()
        .expect("failed to record a single instant")
        .duration_since(instants[0])
        .as_millis();
    println!("total time elapsed was {} milliseconds", total_seconds);
    println!(
        "{} queries per millisecond",
        num_instants as u128 / total_seconds
    );

    Ok(())
}
