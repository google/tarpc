// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::{add::Add as AddService, double::Double as DoubleService};
use futures::{future, prelude::*};
use tarpc::{
    client, context,
    server::{incoming::Incoming, BaseChannel},
};
use tokio_serde::formats::Json;
use tracing_subscriber::prelude::*;

pub mod add {
    #[tarpc::service]
    pub trait Add {
        /// Add two ints together.
        async fn add(x: i32, y: i32) -> i32;
    }
}

pub mod double {
    #[tarpc::service]
    pub trait Double {
        /// 2 * x
        async fn double(x: i32) -> Result<i32, String>;
    }
}

#[derive(Clone)]
struct AddServer;

#[tarpc::server]
impl AddService for AddServer {
    async fn add(self, _: context::Context, x: i32, y: i32) -> i32 {
        x + y
    }
}

#[derive(Clone)]
struct DoubleServer {
    add_client: add::AddClient,
}

#[tarpc::server]
impl DoubleService for DoubleServer {
    async fn double(self, _: context::Context, x: i32) -> Result<i32, String> {
        self.add_client
            .add(context::current(), x, x)
            .await
            .map_err(|e| e.to_string())
    }
}

fn init_tracing(service_name: &str) -> anyhow::Result<()> {
    let tracer = opentelemetry_jaeger::new_pipeline()
        .with_service_name(service_name)
        .with_auto_split_batch(true)
        .with_max_packet_size(2usize.pow(13))
        .install_batch(opentelemetry::runtime::Tokio)?;

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .try_init()?;

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing("tarpc_tracing_example")?;

    let add_listener = tarpc::serde_transport::tcp::listen("localhost:0", Json::default)
        .await?
        .filter_map(|r| future::ready(r.ok()));
    let addr = add_listener.get_ref().local_addr();
    let add_server = add_listener
        .map(BaseChannel::with_defaults)
        .take(1)
        .execute(AddServer.serve());
    tokio::spawn(add_server);

    let to_add_server = tarpc::serde_transport::tcp::connect(addr, Json::default).await?;
    let add_client = add::AddClient::new(client::Config::default(), to_add_server).spawn();

    let double_listener = tarpc::serde_transport::tcp::listen("localhost:0", Json::default)
        .await?
        .filter_map(|r| future::ready(r.ok()));
    let addr = double_listener.get_ref().local_addr();
    let double_server = double_listener
        .map(BaseChannel::with_defaults)
        .take(1)
        .execute(DoubleServer { add_client }.serve());
    tokio::spawn(double_server);

    let to_double_server = tarpc::serde_transport::tcp::connect(addr, Json::default).await?;
    let double_client =
        double::DoubleClient::new(client::Config::default(), to_double_server).spawn();

    let ctx = context::current();
    for _ in 1..=5 {
        tracing::info!("{:?}", double_client.double(ctx, 1).await?);
    }

    opentelemetry::global::shutdown_tracer_provider();

    Ok(())
}
