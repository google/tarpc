// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(plugin)]
#![plugin(tarpc_plugins)]

extern crate env_logger;
extern crate futures;
#[macro_use]
extern crate tarpc;
extern crate tokio_core;

use futures::{Future, future};
use publisher::FutureServiceExt as PublisherExt;
use std::cell::RefCell;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::rc::Rc;
use std::thread;
use std::time::Duration;
use subscriber::FutureServiceExt as SubscriberExt;
use tarpc::future::{client, server};
use tarpc::future::client::ClientExt;
use tarpc::util::{FirstSocketAddr, Message, Never};
use tokio_core::reactor;

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
    type ReceiveFut = Result<(), Never>;

    fn receive(&self, message: String) -> Self::ReceiveFut {
        println!("{} received message: {}", self.id, message);
        Ok(())
    }
}

impl Subscriber {
    fn listen(id: u32,
              handle: &reactor::Handle,
              options: server::Options)
              -> server::Handle {
        let (server_handle, server) = Subscriber { id: id }
            .listen("localhost:0".first_socket_addr(), handle, options)
            .unwrap();
        handle.spawn(server);
        server_handle
    }
}

#[derive(Clone, Debug)]
struct Publisher {
    clients: Rc<RefCell<HashMap<u32, subscriber::FutureClient>>>,
}

impl Publisher {
    fn new() -> Publisher {
        Publisher { clients: Rc::new(RefCell::new(HashMap::new())) }
    }
}

impl publisher::FutureService for Publisher {
    type BroadcastFut = Box<Future<Item = (), Error = Never>>;

    fn broadcast(&self, message: String) -> Self::BroadcastFut {
        let acks = self.clients
                       .borrow()
                       .values()
                       .map(move |client| client.receive(message.clone())
                           // Ignore failing subscribers. In a real pubsub,
                           // you'd want to continually retry until subscribers
                           // ack.
                           .then(|_| Ok(())))
                       // Collect to a vec to end the borrow on `self.clients`.
                       .collect::<Vec<_>>();
        Box::new(future::join_all(acks).map(|_| ()))
    }

    type SubscribeFut = Box<Future<Item = (), Error = Message>>;

    fn subscribe(&self, id: u32, address: SocketAddr) -> Self::SubscribeFut {
        let clients = self.clients.clone();
        Box::new(subscriber::FutureClient::connect(address, client::Options::default())
            .map(move |subscriber| {
                println!("Subscribing {}.", id);
                clients.borrow_mut().insert(id, subscriber);
                ()
            })
            .map_err(|e| e.to_string().into()))
    }

    type UnsubscribeFut = Box<Future<Item = (), Error = Never>>;

    fn unsubscribe(&self, id: u32) -> Self::UnsubscribeFut {
        println!("Unsubscribing {}", id);
        self.clients.borrow_mut().remove(&id).unwrap();
        futures::finished(()).boxed()
    }
}

fn main() {
    let _ = env_logger::init();
    let mut reactor = reactor::Core::new().unwrap();
    let (publisher_handle, server) = Publisher::new()
        .listen("localhost:0".first_socket_addr(),
                &reactor.handle(),
                server::Options::default())
        .unwrap();
    reactor.handle().spawn(server);

    let subscriber1 = Subscriber::listen(0, &reactor.handle(), server::Options::default());
    let subscriber2 = Subscriber::listen(1, &reactor.handle(), server::Options::default());

    let publisher =
        reactor.run(publisher::FutureClient::connect(publisher_handle.addr(),
                                                  client::Options::default()))
            .unwrap();
    reactor.run(publisher.subscribe(0, subscriber1.addr())
            .and_then(|_| publisher.subscribe(1, subscriber2.addr()))
            .map_err(|e| panic!(e))
            .and_then(|_| {
                println!("Broadcasting...");
                publisher.broadcast("hello to all".to_string())
            })
            .and_then(|_| publisher.unsubscribe(1))
            .and_then(|_| publisher.broadcast("hi again".to_string())))
        .unwrap();
    thread::sleep(Duration::from_millis(300));
}
