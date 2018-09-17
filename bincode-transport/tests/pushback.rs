// Copyright 2018 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

//! Tests client/server control flow.

#![feature(generators, await_macro, async_await, futures_api,)]

use futures::{
    compat::{Future01CompatExt, TokioDefaultSpawner},
    prelude::*,
    spawn,
    ready,
};
use log::{error, warn, info, debug, trace};
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
                    "[{}/{}] Responding to request in {:?}.",
                    ctx.trace_id(),
                    client_addr,
                    delay,
                );

                let sleep = Delay::new(Instant::now() + delay).compat();
                async {
                    await!(sleep).unwrap();
                    Ok(request)
                }
            });
            if let Err(e) = spawn!(handler) {
                warn!("Couldn't spawn request handler: {:?}", e);
            }
        });

    spawn!(server).map_err(|e| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("Couldn't spawn server: {:?}", e),
        )
    })?;

    let mut config = client::Config::default();
    config.max_in_flight_requests = 10;
    config.pending_request_buffer = 10;

    let conn = await!(bincode_transport::connect(&addr))?;
    let client = await!(Client::<String, String>::new(config, conn));

    let clients = (1..=100u32).map(|_| client.clone()).collect::<Vec<_>>();
    for mut client in clients {
        let ctx = context::current();
        spawn!(
            async move {
                let trace_id = *ctx.trace_id();
                let response = client.call(ctx, "ping".into());
                match await!(response) {
                    Ok(response) => info!("[{}] response: {}", trace_id, response),
                    Err(e) => error!("[{}] request error: {:?}: {}", trace_id, e.kind(), e),
                }
            }
        ).map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("Couldn't spawn server: {:?}", e),
            )
        })?;
    }

    Ok(())
}

#[test]
fn ping_pong() -> io::Result<()> {
    env_logger::init();

    tokio::run(
        run()
            .map_ok(|_| println!("done"))
            .map_err(|e| panic!(e.to_string()))
            .boxed()
            .compat(TokioDefaultSpawner),
    );

    Ok(())
}
