// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::{add::Add as AddService, double::Double as DoubleService};
use futures::{future, prelude::*};
use std::io;
use tarpc::{
    client, context,
    server::{Handler, Server},
};
use tokio_serde::formats::Json;

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
    async fn double(mut self, _: context::Context, x: i32) -> Result<i32, String> {
        self.add_client
            .add(context::current(), x, x)
            .await
            .map_err(|e| e.to_string())
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    env_logger::init();

    let add_listener = tarpc::serde_transport::tcp::listen("localhost:0", Json::default)
        .await?
        .filter_map(|r| future::ready(r.ok()));
    let addr = add_listener.get_ref().local_addr();
    let add_server = Server::default()
        .incoming(add_listener)
        .take(1)
        .respond_with(AddServer.serve());
    tokio::spawn(add_server);

    let to_add_server = tarpc::serde_transport::tcp::connect(addr, Json::default).await?;
    let add_client = add::AddClient::new(client::Config::default(), to_add_server).spawn()?;

    let double_listener = tarpc::serde_transport::tcp::listen("localhost:0", Json::default)
        .await?
        .filter_map(|r| future::ready(r.ok()));
    let addr = double_listener.get_ref().local_addr();
    let double_server = tarpc::Server::default()
        .incoming(double_listener)
        .take(1)
        .respond_with(DoubleServer { add_client }.serve());
    tokio::spawn(double_server);

    let to_double_server = tarpc::serde_transport::tcp::connect(addr, Json::default).await?;
    let mut double_client =
        double::DoubleClient::new(client::Config::default(), to_double_server).spawn()?;

    for i in 1..=5 {
        eprintln!("{:?}", double_client.double(context::current(), i).await?);
    }
    Ok(())
}
