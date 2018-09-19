// Copyright 2018 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

//! Tests client/server control flow.

#![feature(
    test,
    integer_atomics,
    futures_api,
    generators,
    await_macro,
    async_await
)]

extern crate test;

use self::test::stats::Stats;
use futures::{compat::TokioDefaultSpawner, prelude::*, spawn};
use rpc::{
    client::{self, Client},
    context,
    server::{self, Handler, Server},
};
use std::{
    io,
    time::{Duration, Instant},
};

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
        let response = await!(client.call(context::current(), 0u32));
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

    let (lower, median, upper) = durations_nanos.quartiles();

    println!("Of {} runs:", durations_nanos.len());
    println!("\tSuccessful: {}", successful);
    println!("\tUnsuccessful: {}", unsuccessful);
    println!(
        "\tMean: {:?}",
        Duration::from_nanos(durations_nanos.mean() as u64)
    );
    println!("\tMedian: {:?}", Duration::from_nanos(median as u64));
    println!(
        "\tStd Dev: {:?}",
        Duration::from_nanos(durations_nanos.std_dev() as u64)
    );
    println!(
        "\tMin: {:?}",
        Duration::from_nanos(durations_nanos.min() as u64)
    );
    println!(
        "\tMax: {:?}",
        Duration::from_nanos(durations_nanos.max() as u64)
    );
    println!(
        "\tQuartiles: ({:?}, {:?}, {:?})",
        Duration::from_nanos(lower as u64),
        Duration::from_nanos(median as u64),
        Duration::from_nanos(upper as u64)
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
