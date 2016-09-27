// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(conservative_impl_trait, plugin)]
#![plugin(tarpc_plugins)]

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
use tarpc::future::Connect as Fc;
use tarpc::sync::Connect as Sc;
use tarpc::util::{FirstSocketAddr, Message, Never};

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
}

impl subscriber::FutureService for Subscriber {
    type ReceiveFut = futures::Finished<(), Never>;

    fn receive(&self, message: String) -> Self::ReceiveFut {
        println!("{} received message: {}", self.id, message);
        futures::finished(())
    }
}

impl Subscriber {
    fn new(id: u32) -> tokio::server::ServerHandle {
        Subscriber {
                id: id,
            }
            .listen("localhost:0".first_socket_addr())
            .wait()
            .unwrap()
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
    type BroadcastFut = BoxFuture<(), Never>;

    fn broadcast(&self, message: String) -> Self::BroadcastFut {
        futures::collect(self.clients
                             .lock()
                             .unwrap()
                             .values()
                             // Ignore failing subscribers.
                             .map(move |client| client.receive(message.clone()).then(|_| Ok(())))
                             .collect::<Vec<_>>())
                             .map(|_| ())
                             .boxed()
    }

    type SubscribeFut = BoxFuture<(), Message>;

    fn subscribe(&self, id: u32, address: SocketAddr) -> Self::SubscribeFut {
        let clients = self.clients.clone();
        subscriber::FutureClient::connect(&address)
            .map(move |subscriber| {
                println!("Subscribing {}.", id);
                clients.lock().unwrap().insert(id, subscriber);
                ()
            })
            .map_err(|e| e.to_string().into())
            .boxed()
    }

    type UnsubscribeFut = BoxFuture<(), Never>;

    fn unsubscribe(&self, id: u32) -> Self::UnsubscribeFut {
        println!("Unsubscribing {}", id);
        self.clients.lock().unwrap().remove(&id).unwrap();
        futures::finished(()).boxed()
    }
}

fn main() {
    let _ = env_logger::init();
    let publisher = Publisher::new().listen("localhost:0".first_socket_addr()).wait().unwrap();
    let publisher_addr = publisher.local_addr();
    let publisher = publisher::SyncClient::connect(publisher_addr).unwrap();

    let subscriber1 = Subscriber::new(0);
    publisher.subscribe(0, *subscriber1.local_addr()).unwrap();

    let subscriber2 = Subscriber::new(1);
    publisher.subscribe(1, *subscriber2.local_addr()).unwrap();


    println!("Broadcasting...");
    publisher.broadcast("hello to all".to_string()).unwrap();
    publisher.unsubscribe(1).unwrap();
    publisher.broadcast("hello again".to_string()).unwrap();
    thread::sleep(Duration::from_millis(300));
}
