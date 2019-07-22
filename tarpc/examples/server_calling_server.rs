// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#![feature(existential_type, async_await)]

use crate::{add::Add as AddService, double::Double as DoubleService};
use futures::{
    future::{self, Ready},
    prelude::*,
};
use rpc::{
    client, context,
    server::{Handler, Server},
};
use std::io;

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

impl AddService for AddServer {
    type AddFut = Ready<i32>;

    fn add(self, _: context::Context, x: i32, y: i32) -> Self::AddFut {
        future::ready(x + y)
    }
}

#[derive(Clone)]
struct DoubleServer {
    add_client: add::AddClient,
}

impl DoubleService for DoubleServer {
    existential type DoubleFut: Future<Output = Result<i32, String>> + Send;

    fn double(self, _: context::Context, x: i32) -> Self::DoubleFut {
        async fn double(mut client: add::AddClient, x: i32) -> Result<i32, String> {
            client
                .add(context::current(), x, x)
                .await
                .map_err(|e| e.to_string())
        }

        double(self.add_client.clone(), x)
    }
}

#[runtime::main(runtime_tokio::Tokio)]
async fn main() -> io::Result<()> {
    env_logger::init();

    let add_listener = bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?
        .filter_map(|r| future::ready(r.ok()));
    let addr = add_listener.get_ref().local_addr();
    let add_server = Server::default()
        .incoming(add_listener)
        .take(1)
        .respond_with(AddServer.serve());
    let _ = runtime::spawn(add_server);

    let to_add_server = bincode_transport::connect(&addr).await?;
    let add_client = add::AddClient::new(client::Config::default(), to_add_server).spawn()?;

    let double_listener = bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?
        .filter_map(|r| future::ready(r.ok()));
    let addr = double_listener.get_ref().local_addr();
    let double_server = rpc::Server::default()
        .incoming(double_listener)
        .take(1)
        .respond_with(DoubleServer { add_client }.serve());
    let _ = runtime::spawn(double_server);

    let to_double_server = bincode_transport::connect(&addr).await?;
    let mut double_client =
        double::DoubleClient::new(client::Config::default(), to_double_server).spawn()?;

    for i in 1..=5 {
        eprintln!("{:?}", double_client.double(context::current(), i).await?);
    }
    Ok(())
}
