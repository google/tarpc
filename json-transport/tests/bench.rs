// Copyright 2019 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Tests client/server control flow.

#![feature(test, integer_atomics, async_await)]

use futures::{compat::Executor01CompatExt, prelude::*};
use libtest::stats::Stats;
use rpc::{
    client, context,
    server::{Handler, Server},
};
use std::{
    io,
    time::{Duration, Instant},
};

async fn bench() -> io::Result<()> {
    let listener = tarpc_json_transport::listen(&"0.0.0.0:0".parse().unwrap())?
        .filter_map(|r| future::ready(r.ok()));
    let addr = listener.get_ref().local_addr();

    tokio_executor::spawn(
        Server::<u32, u32>::default()
            .incoming(listener)
            .take(1)
            .respond_with(|_ctx, request| futures::future::ready(Ok(request)))
            .unit_error()
            .boxed()
            .compat(),
    );

    let conn = tarpc_json_transport::connect(&addr).await?;
    let client = &mut client::new::<u32, u32, _>(client::Config::default(), conn).await?;

    let total = 10_000usize;
    let mut successful = 0u32;
    let mut unsuccessful = 0u32;
    let mut durations = vec![];
    for _ in 1..=total {
        let now = Instant::now();
        let response = client.call(context::current(), 0u32).await;
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
    rpc::init(tokio::executor::DefaultExecutor::current().compat());

    tokio::run(bench().map_err(|e| panic!(e.to_string())).boxed().compat());
    println!("done");

    Ok(())
}
