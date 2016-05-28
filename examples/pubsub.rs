// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(default_type_parameter_fallback, try_from)]

#[macro_use]
extern crate tarpc;

use publisher::AsyncServiceExt as PublisherExt;
use subscriber::AsyncServiceExt as SubscriberExt;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tarpc::{Client, Ctx, ServeHandle};

pub mod subscriber {
    service! {
        rpc receive(message: String);
    }
}

pub mod publisher {
    use std::net::SocketAddr;
    service! {
        rpc broadcast(message: String);
        rpc subscribe(id: u32, address: SocketAddr);
        rpc unsubscribe(id: u32);
    }
}

#[derive(Clone, Debug)]
struct Subscriber {
    id: u32,
    publisher: publisher::SyncClient,
}

impl subscriber::AsyncService for Subscriber {
    fn receive(&self, ctx: Ctx<()>, message: String) {
        println!("{} received message: {}", self.id, message);
        ctx.ok(()).unwrap();
    }
}

impl Drop for Subscriber {
    fn drop(&mut self) {
        self.publisher.unsubscribe(&self.id).unwrap();
    }
}

impl Subscriber {
    fn new(id: u32, publisher: publisher::SyncClient) -> ServeHandle {
        let subscriber = Subscriber {
            id: id,
            publisher: publisher.clone()
        }.listen("localhost:0").unwrap();
        publisher.subscribe(&id, &subscriber.local_addr().unwrap()).unwrap();
        subscriber
    }
}

#[derive(Debug)]
struct Publisher {
    clients: Mutex<HashMap<u32, subscriber::AsyncClient>>,
}

impl Publisher {
    fn new() -> Publisher {
        Publisher { clients: Mutex::new(HashMap::new()), }
    }
}

impl publisher::AsyncService for Publisher {
    fn broadcast(&self, ctx: Ctx<()>, message: String) {
        for client in self.clients.lock().unwrap().values() {
            client.receive(|_| {}, &message).unwrap();
        }
        // Returns before all clients are notified. Doesn't retry failed rpc's.
        ctx.reply(Ok(())).unwrap();
    }

    fn subscribe(&self, ctx: Ctx<()>, id: u32, address: SocketAddr) {
        self.clients.lock().unwrap().insert(id, subscriber::AsyncClient::connect(address).unwrap());
        ctx.reply(Ok(())).unwrap();
    }

    fn unsubscribe(&self, ctx: Ctx<()>, id: u32) {
        println!("Unsubscribing {}", id);
        self.clients.lock().unwrap().remove(&id).unwrap();
        ctx.reply(Ok(())).unwrap();
    }
}

fn main() {
    let publisher = Publisher::new().listen("localhost:0").unwrap();
    let publisher = publisher::SyncClient::connect(publisher.local_addr()).unwrap();
    let _subscriber1 = Subscriber::new(0, publisher.clone());
    let subscriber2 = Subscriber::new(1, publisher.clone());
    publisher.broadcast(&"hello to all".to_string()).unwrap();
    thread::sleep(Duration::from_millis(300));
    drop(subscriber2);
    thread::sleep(Duration::from_millis(300));
    publisher.broadcast(&"hello again".to_string()).unwrap();
    thread::sleep(Duration::from_millis(300));
}
