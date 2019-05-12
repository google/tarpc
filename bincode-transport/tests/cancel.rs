// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Tests client/server control flow.

#![feature(async_await)]

use futures::{
    compat::{Executor01CompatExt, Future01CompatExt},
    prelude::*,
    stream::FuturesUnordered,
};
use log::{info, trace};
use rand::distributions::{Distribution, Normal};
use rpc::{
    client, context,
    server::{Channel, Server},
};
use std::{
    io,
    time::{Duration, Instant, SystemTime},
};
use tokio::timer::Delay;

pub trait AsDuration {
    /// Delay of 0 if self is in the past
    fn as_duration(&self) -> Duration;
}

impl AsDuration for SystemTime {
    fn as_duration(&self) -> Duration {
        self.duration_since(SystemTime::now()).unwrap_or_default()
    }
}

async fn run() -> io::Result<()> {
    let listener = tarpc_bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?;
    let addr = listener.local_addr();
    let server = Server::<String, String>::default()
        .incoming(listener)
        .take(1)
        .for_each(async move |channel| {
            let channel = if let Ok(channel) = channel {
                channel
            } else {
                return;
            };
            let client_addr = channel.get_ref().peer_addr().unwrap();
            let handler = channel.respond_with(move |ctx, request| {
                // Sleep for a time sampled from a normal distribution with:
                // - mean: 1/2 the deadline.
                // - std dev: 1/2 the deadline.
                let deadline: Duration = ctx.deadline.as_duration();
                let deadline_millis = deadline.as_secs() * 1000 + deadline.subsec_millis() as u64;
                let distribution =
                    Normal::new(deadline_millis as f64 / 2., deadline_millis as f64 / 2.);
                let delay_millis = distribution.sample(&mut rand::thread_rng()).max(0.);
                let delay = Duration::from_millis(delay_millis as u64);

                trace!(
                    "[{}/{}] Responding to request in {:?}.",
                    ctx.trace_id(),
                    client_addr,
                    delay,
                );

                let wait = Delay::new(Instant::now() + delay).compat();
                async move {
                    wait.await.unwrap();
                    Ok(request)
                }
            });
            tokio_executor::spawn(handler.unit_error().boxed().compat());
        });

    tokio_executor::spawn(server.unit_error().boxed().compat());

    let conn = tarpc_bincode_transport::connect(&addr).await?;
    let client = client::new::<String, String, _>(client::Config::default(), conn).await?;

    // Proxy service
    let listener = tarpc_bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?;
    let addr = listener.local_addr();
    let proxy_server = Server::<String, String>::default()
        .incoming(listener)
        .take(1)
        .for_each(move |channel| {
            let client = client.clone();
            async move {
                let channel = if let Ok(channel) = channel {
                    channel
                } else {
                    return;
                };
                let client_addr = channel.get_ref().peer_addr().unwrap();
                let handler = channel.respond_with(move |ctx, request| {
                    trace!("[{}/{}] Proxying request.", ctx.trace_id(), client_addr);
                    let mut client = client.clone();
                    async move { client.call(ctx, request).await }
                });
                tokio_executor::spawn(handler.unit_error().boxed().compat());
            }
        });

    tokio_executor::spawn(proxy_server.unit_error().boxed().compat());

    let mut config = client::Config::default();
    config.max_in_flight_requests = 10;
    config.pending_request_buffer = 10;

    let client =
        client::new::<String, String, _>(config, tarpc_bincode_transport::connect(&addr).await?)
            .await?;

    // Make 3 speculative requests, returning only the quickest.
    let mut clients: Vec<_> = (1..=3u32).map(|_| client.clone()).collect();
    let mut requests = vec![];
    for client in &mut clients {
        let mut ctx = context::current();
        ctx.deadline = SystemTime::now() + Duration::from_millis(200);
        let trace_id = *ctx.trace_id();
        let response = client.call(ctx, "ping".into());
        requests.push(response.map(move |r| (trace_id, r)));
    }
    let (fastest_response, _) = requests
        .into_iter()
        .collect::<FuturesUnordered<_>>()
        .into_future()
        .await;
    let (trace_id, resp) = fastest_response.unwrap();
    info!("[{}] fastest_response = {:?}", trace_id, resp);

    Ok::<_, io::Error>(())
}

#[test]
fn cancel_slower() -> io::Result<()> {
    env_logger::init();
    rpc::init(tokio::executor::DefaultExecutor::current().compat());

    tokio::run(run().boxed().map_err(|e| panic!(e)).compat());
    Ok(())
}
