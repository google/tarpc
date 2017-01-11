// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(conservative_impl_trait, plugin)]
#![plugin(tarpc_plugins)]

extern crate bincode;
extern crate env_logger;
extern crate futures;
#[macro_use]
extern crate log;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate tarpc;
extern crate tokio_core;
extern crate tokio_service;

use bincode::serde::DeserializeError;
use futures::{Future, IntoFuture};
use std::io;
use std::net::SocketAddr;
use tarpc::future::Connect;
use tarpc::util::FirstSocketAddr;
use tarpc::util::Never;
use tarpc::{Listen, ClientConfig, ServerConfig};
use tokio_service::Service;

#[derive(Clone, Copy)]
struct HelloServer;

impl HelloServer {
    fn listen(addr: SocketAddr) -> impl Future<Item=SocketAddr, Error=io::Error> {
        let (tx, rx) = futures::oneshot();
        tarpc::future::REMOTE.spawn(move |handle| {
            Ok(tx.complete(ServerConfig::new_tcp().listen_with(addr, move || Ok(HelloServer), handle.clone())))
        });
        rx.map_err(|e| panic!(e)).and_then(|result| result)
    }
}

impl Service for HelloServer {
    type Request = Result<String, DeserializeError>;
    type Response = tarpc::Response<String, Never>;
    type Error = io::Error;
    type Future = Box<Future<Item = tarpc::Response<String, Never>, Error = io::Error>>;

    fn call(&self, request: Self::Request) -> Self::Future {
        Ok(Ok(format!("Hello, {}!", request.unwrap()))).into_future().boxed()
    }
}

/// The client stub that makes RPC calls to the server. Exposes a Future interface.
#[derive(Debug)]
pub struct FutureClient(tarpc::Client<String, String, Never>);

impl FutureClient {
    fn connect(addr: &SocketAddr) -> impl Future<Item = FutureClient, Error = io::Error> {
        tarpc::Client::connect_remotely(addr, &tarpc::future::REMOTE, ClientConfig::new_tcp()).map(FutureClient)
    }

    pub fn hello(&mut self, name: String)
        -> impl Future<Item = String, Error = tarpc::Error<Never>> + 'static
    {
        self.0.call(name).then(|msg| msg.unwrap())
    }
}

fn main() {
    let _ = env_logger::init();
    let mut core = tokio_core::reactor::Core::new().unwrap();
    let addr = HelloServer::listen("localhost:10000".first_socket_addr()).wait().unwrap();
    let f = FutureClient::connect(&addr)
        .map_err(tarpc::Error::from)
        .and_then(|mut client| {
            let resp1 = client.hello("Mom".to_string());
            info!("Sent first request.");

            let resp2 = client.hello("Dad".to_string());
            info!("Sent second request.");

            futures::collect(vec![resp1, resp2])
        })
        .map(|responses| {
            for resp in responses {
                println!("{}", resp);
            }
        });
    core.run(f).unwrap();
}
