// Copyright 2018 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

//! Tests client/server control flow.

#![feature(generators, await_macro, async_await, futures_api,)]

#[macro_use]
extern crate log;
#[macro_use]
extern crate futures;

use futures::{
    compat::{Future01CompatExt, TokioDefaultSpawner},
    prelude::*,
    stream,
};
use humantime::format_duration;
use rand::distributions::{Distribution, Normal};
use rpc::{
    client::{self, Client},
    context,
    server::{self, Server},
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
    let listener = bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?;
    let addr = listener.local_addr();
    let server = Server::<String, String>::new(server::Config::default())
        .incoming(listener)
        .take(1)
        .for_each(async move |channel| {
            let channel = if let Ok(channel) = channel {
                channel
            } else {
                return;
            };
            let client_addr = *channel.client_addr();
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
                    "[{}/{}] Responding to request in {}.",
                    ctx.trace_id(),
                    client_addr,
                    format_duration(delay),
                );

                let wait = Delay::new(Instant::now() + delay).compat();
                async move {
                    await!(wait).unwrap();
                    Ok(request)
                }
            });
            spawn!(handler);
        });

    spawn!(server).map_err(|e| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("Couldn't spawn server: {:?}", e),
        )
    })?;

    let conn = await!(bincode_transport::connect(&addr))?;
    let client = await!(Client::<String, String>::new(
        client::Config::default(),
        conn
    ));

    // Proxy service
    let listener = bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?;
    let addr = listener.local_addr();
    let proxy_server = Server::<String, String>::new(server::Config::default())
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
                let client_addr = *channel.client_addr();
                let handler = channel.respond_with(move |ctx, request| {
                    trace!("[{}/{}] Proxying request.", ctx.trace_id(), client_addr);
                    let mut client = client.clone();
                    async move { await!(client.call(ctx, request)) }
                });
                spawn!(handler);
            }
        });

    spawn!(proxy_server).map_err(|e| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("Couldn't spawn server: {:?}", e),
        )
    })?;

    let mut config = client::Config::default();
    config.max_in_flight_requests = 10;
    config.pending_request_buffer = 10;

    let client = await!(Client::<String, String>::new(
        config,
        await!(bincode_transport::connect(&addr))?
    ));

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
    let (fastest_response, _) = await!(stream::futures_unordered(requests).into_future());
    let (trace_id, resp) = fastest_response.unwrap();
    info!("[{}] fastest_response = {:?}", trace_id, resp);

    Ok::<_, io::Error>(())
}

#[test]
fn cancel_slower() -> io::Result<()> {
    env_logger::init();

    tokio::run(
        run()
            .boxed()
            .map_err(|e| panic!(e))
            .compat(TokioDefaultSpawner),
    );

    Ok(())
}
