// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(
    plugin,
    futures_api,
    await_macro,
    async_await,
    existential_type
)]
#![plugin(tarpc_plugins)]

use crate::{add::Service as AddService, double::Service as DoubleService};
use futures::{
    compat::TokioDefaultSpawner,
    future::{self, Ready},
    prelude::*,
    spawn,
};
use rpc::{
    client, context,
    server::{self, Handler, Server},
};
use std::io;

pub mod add {
    tarpc::service! {
        /// Add two ints together.
        rpc add(x: i32, y: i32) -> i32;
    }
}

pub mod double {
    tarpc::service! {
        /// 2 * x
        rpc double(x: i32) -> Result<i32, String>;
    }
}

#[derive(Clone)]
struct AddServer;

impl AddService for AddServer {
    type AddFut = Ready<i32>;

    fn add(&self, _: context::Context, x: i32, y: i32) -> Self::AddFut {
        future::ready(x + y)
    }
}

#[derive(Clone)]
struct DoubleServer {
    add_client: add::Client,
}

impl DoubleService for DoubleServer {
    existential type DoubleFut: Future<Output = Result<i32, String>> + Send;

    fn double(&self, _: context::Context, x: i32) -> Self::DoubleFut {
        async fn double(mut client: add::Client, x: i32) -> Result<i32, String> {
            let result = await!(client.add(context::current(), x, x));
            result.map_err(|e| e.to_string())
        }

        double(self.add_client.clone(), x)
    }
}

async fn run() -> io::Result<()> {
    let add_listener = bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?;
    let addr = add_listener.local_addr();
    let add_server = Server::new(server::Config::default())
        .incoming(add_listener)
        .take(1)
        .respond_with(add::serve(AddServer));
    spawn!(add_server);

    let to_add_server = await!(bincode_transport::connect(&addr))?;
    let add_client = await!(add::new_stub(client::Config::default(), to_add_server));

    let double_listener = bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?;
    let addr = double_listener.local_addr();
    let double_server = rpc::Server::new(server::Config::default())
        .incoming(double_listener)
        .take(1)
        .respond_with(double::serve(DoubleServer { add_client }));
    spawn!(double_server);

    let to_double_server = await!(bincode_transport::connect(&addr))?;
    let mut double_client = await!(double::new_stub(
        client::Config::default(),
        to_double_server
    ));

    for i in 1..=5 {
        println!("{:?}", await!(double_client.double(context::current(), i))?);
    }
    Ok(())
}

fn main() {
    env_logger::init();
    tokio::run(
        run()
            .map_err(|e| panic!(e))
            .boxed()
            .compat(TokioDefaultSpawner),
    );
}
