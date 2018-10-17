// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#![feature(
    test,
    arbitrary_self_types,
    pin,
    integer_atomics,
    futures_api,
    generators,
    await_macro,
    async_await,
    proc_macro_hygiene,
)]

extern crate test;

use self::test::stats::Stats;
use futures::{compat::TokioDefaultSpawner, future, prelude::*};
use rpc::{
    client, context,
    server::{self, Handler, Server},
};
use std::{
    io,
    time::{Duration, Instant},
};

mod ack {
    tarpc::service! {
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

    tokio_executor::spawn(
        Server::new(server::Config::default())
            .incoming(listener)
            .take(1)
            .respond_with(ack::serve(Serve))
            .unit_error()
            .boxed()
            .compat()
    );

    let conn = await!(bincode_transport::connect(&addr))?;
    let mut client = await!(ack::new_stub(client::Config::default(), conn))?;

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

    let (lower, median, upper) = durations_nanos.quartiles();

    println!("Of {:?} runs:", durations_nanos.len());
    println!("\tSuccessful: {:?}", successful);
    println!("\tUnsuccessful: {:?}", unsuccessful);
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

    println!("done");
    Ok(())
}

#[test]
fn bench_small_packet() {
    env_logger::init();
    tarpc::init(TokioDefaultSpawner);

    tokio::run(
        bench()
            .map_err(|e| panic!(e.to_string()))
            .boxed()
            .compat(),
    )
}
