//! Tests client/server control flow.

#![feature(
    test,
    integer_atomics,
    futures_api,
    generators,
    await_macro,
    async_await
)]

#[macro_use]
extern crate futures;

use futures::compat::TokioDefaultSpawner;
use futures::prelude::*;
use humantime::{format_duration, FormattedDuration};
use rpc::{
    client::{self, Client},
    server::{self, Handler, Server},
};
use std::{
    io,
    time::{Duration, Instant},
};
use test::stats::Stats;

async fn bench() -> io::Result<()> {
    let listener = bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?;
    let addr = listener.local_addr();

    spawn!(
        Server::<u32, u32>::new(server::Config::default())
            .incoming(listener)
            .take(1)
            .respond_with(|_ctx, request| futures::future::ready(Ok(request)))
    );

    let conn = await!(bincode_transport::connect(&addr))?;
    let client = &mut await!(Client::<u32, u32>::new(client::Config::default(), conn));

    let total = 10_000usize;
    let mut successful = 0u32;
    let mut unsuccessful = 0u32;
    let mut durations = vec![];
    for _ in 1..=total {
        let now = Instant::now();
        let response = await!(client.send(client::Context::current(), 0u32));
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

    Ok(())
}

#[test]
fn bench_small_packet() -> io::Result<()> {
    env_logger::init();
    tokio::run(
        bench()
            .map_err(|e| panic!(e.to_string()))
            .boxed()
            .compat(TokioDefaultSpawner),
    );
    println!("done");

    Ok(())
}
