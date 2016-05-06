// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#[macro_use]
extern crate tarpc;

extern crate env_logger;
#[macro_use]
extern crate log;

extern crate mio;
use mio::*;
use mio::tcp::TcpListener;

use std::net::ToSocketAddrs;
use std::time::{Duration, Instant};
use tarpc::protocol::async::server;
use tarpc::protocol::client::Packet;

service! {
    rpc hello(buf: Vec<u8>) -> Vec<u8>;
}

struct HelloServer;
impl Service for HelloServer {
    fn hello(&self, buf: Vec<u8>) -> Vec<u8> {
        buf
    }
}

impl server::Service for HelloServer {
    fn handle(&mut self, token: Token, packet: Packet, event_loop: &mut EventLoop<server::Dispatcher>) {
        event_loop.channel().send(server::Action::Reply(token, packet)).unwrap();
    }
}

fn main() {
    let _ = env_logger::init();
    let addr = "127.0.0.1:58765".to_socket_addrs().unwrap().next().unwrap();
    /*
    let socket = TcpListener::bind(&addr).unwrap();
    let register = server::Dispatcher::spawn();
    register.register(server::Server::new(socket, Box::new(HelloServer))).unwrap();
    */
    let handle = HelloServer.spawn(addr).unwrap();
    let client = FutureClient::new(&addr).unwrap();
    let concurrency = 100;
    let mut futures = Vec::with_capacity(concurrency);

    info!("Starting...");
    let start = Instant::now();
    let max = Duration::from_secs(10);
    let mut total_rpcs = 0;
    let buf = vec![1; 1028];

    while start.elapsed() < max {
        for _ in 0..concurrency {
            futures.push(client.hello(&buf));
        }
        for f in futures.drain(..) {
            f.get().unwrap();
        }
        total_rpcs += concurrency;
    }
    info!("Done. Total rpcs in 10s: {}", total_rpcs);
    client.shutdown().unwrap();
    //register.shutdown().unwrap();
    handle.shutdown();
}
