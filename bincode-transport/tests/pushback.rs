//! Tests client/server control flow.

#![feature(generators, await_macro, async_await, futures_api,)]

#[macro_use]
extern crate log;
#[macro_use]
extern crate futures;

use futures::compat::TokioDefaultSpawner;
use futures::compat::{Future01CompatExt, Stream01CompatExt};
use futures::prelude::*;
use humantime::format_duration;
use rand::distributions::{Distribution, Normal};
use rpc::{
    client::{self, Client},
    server::{self, Handler, Server},
};
use std::{
    io,
    time::{Duration, Instant, SystemTime},
};
use tokio::{
    net::{TcpListener, TcpStream},
    timer::Delay,
};

pub trait AsDuration {
    /// Delay of 0 if self is in the past
    fn as_duration(&self) -> Duration;
}

impl AsDuration for SystemTime {
    fn as_duration(&self) -> Duration {
        self.duration_since(SystemTime::now()).unwrap_or_default()
    }
}

#[test]
fn ping_pong() -> Result<(), io::Error> {
    env_logger::init();

    let listener = TcpListener::bind(&"0.0.0.0:0".parse().unwrap())?;
    let addr = listener.local_addr()?;
    let server = Server::<String, String>::new(server::Config::default())
        .incoming(
            listener
                .incoming()
                .compat()
                .take(1)
                .map_ok(bincode_transport::new),
        ).respond_with(|ctx, request| {
            let client_addr = ctx.client_addr;

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

            let sleep = Delay::new(Instant::now() + delay).compat();
            async {
                await!(sleep).unwrap();
                Ok(request)
            }
        });

    let client = async move {
        let mut config = client::Config::default();
        config.max_in_flight_requests = 10;
        config.pending_request_buffer = 10;

        let client = await!(Client::<String, String>::new(
            config,
            bincode_transport::new(await!(TcpStream::connect(&addr).compat())?)
        ));

        let clients = (1..=100u32).map(|_| client.clone()).collect::<Vec<_>>();
        for mut client in clients {
            let ctx = client::Context::current();

            spawn!(
                async move {
                    let trace_id = *ctx.trace_id();
                    let response = client.send(ctx, "ping");
                    match await!(response) {
                        Ok(response) => info!("[{}] response: {}", trace_id, response),
                        Err(e) => error!("[{}] request error: {:?}: {}", trace_id, e.kind(), e),
                    }
                }
            );
        }

        Ok::<_, io::Error>(())
    };

    tokio::run(
        server
            .join(client)
            .map(|_| println!("done"))
            .boxed()
            .unit_error()
            .compat(TokioDefaultSpawner),
    );

    Ok(())
}
