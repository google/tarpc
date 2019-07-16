// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#![feature(
    arbitrary_self_types,
    async_await,
    existential_type,
    proc_macro_hygiene
)]

use futures::{
    future::{self, Ready},
    prelude::*,
    Future,
};
use rpc::{
    client, context,
    server::{self, Handler},
};
use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

pub mod subscriber {
    tarpc::service! {
        rpc receive(message: String);
    }
}

pub mod publisher {
    use std::net::SocketAddr;
    tarpc::service! {
        rpc broadcast(message: String);
        rpc subscribe(id: u32, address: SocketAddr) -> Result<(), String>;
        rpc unsubscribe(id: u32);
    }
}

#[derive(Clone, Debug)]
struct Subscriber {
    id: u32,
}

impl subscriber::Service for Subscriber {
    type ReceiveFut = Ready<()>;

    fn receive(self, _: context::Context, message: String) -> Self::ReceiveFut {
        println!("{} received message: {}", self.id, message);
        future::ready(())
    }
}

impl Subscriber {
    async fn listen(id: u32, config: server::Config) -> io::Result<SocketAddr> {
        let incoming = bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?
            .filter_map(|r| future::ready(r.ok()));
        let addr = incoming.get_ref().local_addr();
        tokio_executor::spawn(
            server::new(config)
                .incoming(incoming)
                .take(1)
                .respond_with(subscriber::serve(Subscriber { id }))
                .unit_error()
                .boxed()
                .compat(),
        );
        Ok(addr)
    }
}

#[derive(Clone, Debug)]
struct Publisher {
    clients: Arc<Mutex<HashMap<u32, subscriber::Client>>>,
}

impl Publisher {
    fn new() -> Publisher {
        Publisher {
            clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl publisher::Service for Publisher {
    existential type BroadcastFut: Future<Output = ()>;

    fn broadcast(self, _: context::Context, message: String) -> Self::BroadcastFut {
        async fn broadcast(clients: Arc<Mutex<HashMap<u32, subscriber::Client>>>, message: String) {
            let mut clients = clients.lock().unwrap().clone();
            for client in clients.values_mut() {
                // Ignore failing subscribers. In a real pubsub,
                // you'd want to continually retry until subscribers
                // ack.
                let _ = client.receive(context::current(), message.clone()).await;
            }
        }

        broadcast(self.clients.clone(), message)
    }

    existential type SubscribeFut: Future<Output = Result<(), String>>;

    fn subscribe(self, _: context::Context, id: u32, addr: SocketAddr) -> Self::SubscribeFut {
        async fn subscribe(
            clients: Arc<Mutex<HashMap<u32, subscriber::Client>>>,
            id: u32,
            addr: SocketAddr,
        ) -> io::Result<()> {
            let conn = bincode_transport::connect(&addr).await?;
            let subscriber = subscriber::new_stub(client::Config::default(), conn).await?;
            println!("Subscribing {}.", id);
            clients.lock().unwrap().insert(id, subscriber);
            Ok(())
        }

        subscribe(Arc::clone(&self.clients), id, addr).map_err(|e| e.to_string())
    }

    existential type UnsubscribeFut: Future<Output = ()>;

    fn unsubscribe(self, _: context::Context, id: u32) -> Self::UnsubscribeFut {
        println!("Unsubscribing {}", id);
        let mut clients = self.clients.lock().unwrap();
        if let None = clients.remove(&id) {
            eprintln!(
                "Client {} not found. Existings clients: {:?}",
                id, &*clients
            );
        }
        future::ready(())
    }
}

async fn run() -> io::Result<()> {
    env_logger::init();
    let transport = bincode_transport::listen(&"0.0.0.0:0".parse().unwrap())?
        .filter_map(|r| future::ready(r.ok()));
    let publisher_addr = transport.get_ref().local_addr();
    tokio_executor::spawn(
        transport
            .take(1)
            .map(server::BaseChannel::with_defaults)
            .respond_with(publisher::serve(Publisher::new()))
            .unit_error()
            .boxed()
            .compat(),
    );

    let subscriber1 = Subscriber::listen(0, server::Config::default()).await?;
    let subscriber2 = Subscriber::listen(1, server::Config::default()).await?;

    let publisher_conn = bincode_transport::connect(&publisher_addr);
    let publisher_conn = publisher_conn.await?;
    let mut publisher = publisher::new_stub(client::Config::default(), publisher_conn).await?;

    if let Err(e) = publisher
        .subscribe(context::current(), 0, subscriber1)
        .await?
    {
        eprintln!("Couldn't subscribe subscriber 0: {}", e);
    }
    if let Err(e) = publisher
        .subscribe(context::current(), 1, subscriber2)
        .await?
    {
        eprintln!("Couldn't subscribe subscriber 1: {}", e);
    }

    println!("Broadcasting...");
    publisher
        .broadcast(context::current(), "hello to all".to_string())
        .await?;
    publisher.unsubscribe(context::current(), 1).await?;
    publisher
        .broadcast(context::current(), "hi again".to_string())
        .await?;
    Ok(())
}

fn main() {
    tokio::run(run().boxed().map_err(|e| panic!(e)).boxed().compat());
    thread::sleep(Duration::from_millis(100));
}
