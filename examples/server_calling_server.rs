// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(plugin, futures_api, await_macro, async_await, existential_type)]
#![plugin(tarpc_plugins)]

#[macro_use]
extern crate tarpc;
#[macro_use]
extern crate futures;

use crate::add::{Service as AddService};
use crate::double::{Service as DoubleService};
use futures::{prelude::*,
    future::{self, Ready},
};
use futures::compat::TokioDefaultSpawner;
use rpc::{client, server};
use std::io;

pub mod add {
    service! {
        /// Add two ints together.
        rpc add(x: i32, y: i32) -> i32;
    }
}

pub mod double {
    service! {
        /// 2 * x
        rpc double(x: i32) -> Result<i32, String>;
    }
}

#[derive(Clone)]
struct AddServer;

impl AddService for AddServer {
    type AddFut = Ready<i32>;

    fn add(&self, _: &server::Context, x: i32, y: i32) -> Self::AddFut {
        future::ready(x + y)
    }
}

#[derive(Clone)]
struct DoubleServer {
    client: add::Client,
}

impl DoubleServer {
    fn new(client: add::Client) -> Self {
        DoubleServer { client: client }
    }
}

existential type Double: Future<Output = Result<i32, String>> + Send;

impl DoubleService for DoubleServer {
    type DoubleFut = Double;

    fn double(&self, _: &server::Context, x: i32) -> Self::DoubleFut {
        let client = self.client.clone();
        async move {
            let result = await!(client.add(client::Context::current(), x, x));
             result.map_err(|e| e.to_string())
        }
    }
}

async fn run() -> io::Result<()> {
    let transport = bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?;
    let addr = transport.local_addr();
    let add_server = rpc::Server::new(server::Config::default())
        .incoming(transport)
        .respond_with(add::serve(AddServer));
    spawn!(add_server);

    let transport = await!(bincode_transport::connect(&addr))?;
    let mut add_client = await!(add::newStub(client::Config::default(), transport));

    let transport = bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?;
    let addr = transport.local_addr();
    let double_server = rpc::Server::new(server::Config::default())
        .incoming(transport)
        .respond_with(double::serve(DoubleServer{client: add_client}));
    spawn!(double_server);

    let transport = await!(bincode_transport::connect(&addr))?;
    let mut double_client = await!(double::newStub(client::Config::default(), transport));

    for i in 0i32..5 {
        println!("{:?}", await!(double_client.double(client::Context::current(), i))?);
    }
    Ok(())
}

fn main() {
    env_logger::init();
    tokio::run(run().map_err(|e| panic!(e)).boxed().compat(TokioDefaultSpawner));
}
