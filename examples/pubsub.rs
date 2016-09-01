// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(conservative_impl_trait)]

extern crate env_logger;
extern crate futures;
#[macro_use]
extern crate tarpc;
extern crate tokio_proto as tokio;

use futures::{BoxFuture, Future};
use publisher::FutureServiceExt as PublisherExt;
use subscriber::FutureServiceExt as SubscriberExt;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tarpc::util::{Never, Message, Spawn};
use tarpc::future::Connect as Fc;
use tarpc::sync::Connect as Sc;

pub mod subscriber {
    service! {
        rpc receive(message: String);
    }
}

pub mod publisher {
    use std::net::SocketAddr;
    use tarpc::util::Message;

    service! {
        rpc broadcast(message: String);
        rpc subscribe(id: u32, address: SocketAddr) | Message;
        rpc unsubscribe(id: u32);
    }
}

#[derive(Clone, Debug)]
struct Subscriber {
    id: u32,
    publisher: publisher::SyncClient,
}

impl subscriber::FutureService for Subscriber {
    fn receive(&self, message: String) -> BoxFuture<(), Never> {
        println!("{} received message: {}", self.id, message);
        futures::finished(()).boxed()
    }
}

impl Subscriber {
    fn new(id: u32, publisher: publisher::SyncClient) -> tokio::server::ServerHandle {
        let subscriber = Subscriber {
                id: id,
                publisher: publisher.clone(),
            }
            .listen("localhost:0")
            .unwrap();
        publisher.subscribe(&id, &subscriber.local_addr()).unwrap();
        subscriber
    }
}

#[derive(Clone, Debug)]
struct Publisher {
    clients: Arc<Mutex<HashMap<u32, subscriber::FutureClient>>>,
}

impl Publisher {
    fn new() -> Publisher {
        Publisher { clients: Arc::new(Mutex::new(HashMap::new())) }
    }
}

impl publisher::FutureService for Publisher {
    fn broadcast(&self, message: String) -> BoxFuture<(), Never> {
        for client in self.clients.lock().unwrap().values() {
            client.receive(&message).spawn();
        }
        // Returns before all clients are notified. Doesn't retry failed rpc's.
        futures::finished(()).boxed()
    }

    fn subscribe(&self, id: u32, address: SocketAddr) -> BoxFuture<(), Message> {
        let clients = self.clients.clone();
        subscriber::FutureClient::connect(address)
            .map(move |subscriber| {
                println!("Subscribing {}.", id);
                clients.lock().unwrap().insert(id, subscriber);
                ()
            })
            .map_err(|e| e.to_string().into())
            .boxed()
    }

    fn unsubscribe(&self, id: u32) -> BoxFuture<(), Never> {
        println!("Unsubscribing {}", id);
        self.clients.lock().unwrap().remove(&id).unwrap();
        futures::finished(()).boxed()
    }
}

fn main() {
    let _ = env_logger::init();
    let publisher = Publisher::new().listen("localhost:0").unwrap();
    let publisher = publisher::SyncClient::connect(publisher.local_addr()).unwrap();
    let _subscriber1 = Subscriber::new(0, publisher.clone());
    let _subscriber2 = Subscriber::new(1, publisher.clone());

    println!("Broadcasting...");
    publisher.broadcast(&"hello to all".to_string()).unwrap();
    publisher.unsubscribe(&1).unwrap();
    publisher.broadcast(&"hello again".to_string()).unwrap();
    thread::sleep(Duration::from_millis(300));
}
