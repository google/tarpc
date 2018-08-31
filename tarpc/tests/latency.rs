#![feature(
    plugin,
    test,
    integer_atomics,
    futures_api,
    generators,
    await_macro,
    async_await,
    existential_type,
)]
#![plugin(tarpc_plugins)]

#[macro_use]
extern crate futures;
#[macro_use]
extern crate tarpc;

use futures::compat::TokioDefaultSpawner;
use futures::{future, prelude::*};
use humantime::{format_duration, FormattedDuration};
use rpc::{
    context,
    client,
    server::{self, Handler, Server},
};
use std::{
    io,
    time::{Duration, Instant},
};
use test::stats::Stats;

mod ack {
    service! {
        rpc ack();
    }
}

#[derive(Clone)]
struct Serve;

impl ack::Service for Serve {
    type AckFut = future::Ready<()>;

    fn ack(&self, _: context::Context) -> Self::AckFut {
        future::ready(())
    }
}

async fn bench() -> io::Result<()> {
    let listener = bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?;
    let addr = listener.local_addr();

    spawn!(
        Server::new(server::Config::default())
            .incoming(listener)
            .take(1)
            .respond_with(ack::serve(Serve))
    );

    let conn = await!(bincode_transport::connect(&addr))?;
    let mut client = await!(ack::new_stub(client::Config::default(), conn));

    let total = 10_000usize;
    let mut successful = 0u32;
    let mut unsuccessful = 0u32;
    let mut durations = vec![];
    for _ in 1..=total {
        let now = Instant::now();
        let response = await!(client.ack(context::current()));
        let elapsed = now.elapsed();

        match response {
            Ok(_) => successful += 1,
            Err(_) => unsuccessful += 1,
        };
        durations.push(elapsed);
    }

    let durations_nanos = durations
        .iter()
        .map(|duration| duration.as_secs() as f64 * 1E9 + duration.subsec_nanos() as f64)
        .collect::<Vec<_>>();

    fn format_nanos(nanos: f64) -> FormattedDuration {
        format_duration(Duration::from_nanos(nanos as u64))
    }

    let (lower, median, upper) = durations_nanos.quartiles();

    println!("Of {} runs:", durations_nanos.len());
    println!("\tSuccessful: {}", successful);
    println!("\tUnsuccessful: {}", unsuccessful);
    println!("\tMean: {}", format_nanos(durations_nanos.mean()));
    println!("\tMedian: {}", format_nanos(median));
    println!("\tStd Dev: {}", format_nanos(durations_nanos.std_dev()));
    println!("\tMin: {}", format_nanos(durations_nanos.min()));
    println!("\tMax: {}", format_nanos(durations_nanos.max()));
    println!(
        "\tQuartiles: ({}, {}, {})",
        format_nanos(lower),
        format_nanos(median),
        format_nanos(upper)
    );

    println!("done");
    Ok(())
}

#[test]
fn bench_small_packet() {
    env_logger::init();

    tokio::run(
        bench()
            .map_err(|e| panic!(e.to_string()))
            .boxed()
            .compat(TokioDefaultSpawner),
    )
}
