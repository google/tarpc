// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::{add::Add as AddService, double::Double as DoubleService};
use futures::{
    future::{self, Ready},
    prelude::*,
};
use std::{io, pin::Pin};
use tarpc::{
    client, context,
    server::{Handler, Server},
};

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
    type DoubleFut = Pin<Box<dyn Future<Output = Result<i32, String>> + Send>>;

    fn double(self, _: context::Context, x: i32) -> Self::DoubleFut {
        async fn double(mut client: add::AddClient, x: i32) -> Result<i32, String> {
            client
                .add(context::current(), x, x)
                .await
                .map_err(|e| e.to_string())
        }

        double(self.add_client.clone(), x).boxed()
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    env_logger::init();

    let add_listener = tarpc_bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?
        .filter_map(|r| future::ready(r.ok()));
    let addr = add_listener.get_ref().local_addr();
    let add_server = Server::default()
        .incoming(add_listener)
        .take(1)
        .respond_with(AddServer.serve());
    tokio::spawn(add_server);

    let to_add_server = tarpc_bincode_transport::connect(&addr).await?;
    let add_client = add::AddClient::new(client::Config::default(), to_add_server).spawn()?;

    let double_listener = tarpc_bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?
        .filter_map(|r| future::ready(r.ok()));
    let addr = double_listener.get_ref().local_addr();
    let double_server = rpc::Server::default()
        .incoming(double_listener)
        .take(1)
        .respond_with(DoubleServer { add_client }.serve());
    tokio::spawn(double_server);

    let to_double_server = tarpc_bincode_transport::connect(&addr).await?;
    let mut double_client =
        double::DoubleClient::new(client::Config::default(), to_double_server).spawn()?;

    for i in 1..=5 {
        eprintln!("{:?}", double_client.double(context::current(), i).await?);
    }
    Ok(())
}
