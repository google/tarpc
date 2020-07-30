// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

/// - The PubSub server sets up TCP listeners on 2 ports, the "subscriber" port and the "publisher"
///   port. Because both publishers and subscribers initiate their connections to the PubSub
///   server, the server requires no prior knowledge of either publishers or subscribers.
///
/// - Subscribers connect to the server on the server's "subscriber" port. Once the connection is
///   established, the server acts as the client, sending messages to the subscribers via a
///   Subscriber service.
///
/// - Publishers connect to the server on the "publisher"port, and once connected, they send
///   messages to the server to be broadcast via a Publisher service.
///
///       Subscriber                        Publisher                       PubSub Server
/// T1        |                                 |                                 |             
/// T2        |-----Connect------------------------------------------------------>|
/// T3        |                                 |                                 |
/// T4        |                                 |-----Connect-------------------->|
/// T5        |                                 |                                 |
/// T6        |                                 |-----Publish-------------------->|
/// T7        |                                 |                                 |
/// T8        |<------------------------------------------------------Receive-----|
/// T9        |-----(Receive OK)------------------------------------------------->|
/// T10       |                                 |                                 |
/// T11       |                                 |<--------------(Publish OK)------|

use futures::{
    future::{self, AbortHandle},
    prelude::*,
};
use log::info;
use publisher::Publisher as _;
use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use subscriber::Subscriber as _;
use tarpc::{
    client, context,
    serde_transport::tcp,
    server::{self, Channel},
};
use tokio::net::ToSocketAddrs;
use tokio_serde::formats::Json;

pub mod subscriber {
    #[tarpc::service]
    pub trait Subscriber {
        async fn receive(message: String);
    }
}

pub mod publisher {
    #[tarpc::service]
    pub trait Publisher {
        async fn broadcast(message: String);
    }
}

#[derive(Clone, Debug)]
struct Subscriber {
    local_addr: SocketAddr,
}

#[tarpc::server]
impl subscriber::Subscriber for Subscriber {
    async fn receive(self, _: context::Context, message: String) {
        info!("[{}] received message: {}", self.local_addr, message);
    }
}

struct SubscriberHandle(AbortHandle);

impl Drop for SubscriberHandle {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Subscriber {
    async fn connect(publisher_addr: impl ToSocketAddrs) -> io::Result<SubscriberHandle> {
        let publisher = tcp::connect(publisher_addr, Json::default()).await?;
        let local_addr = publisher.local_addr()?;
        let (handler, abort_handle) = future::abortable(
            server::BaseChannel::with_defaults(publisher)
                .respond_with(Subscriber { local_addr }.serve())
                .execute(),
        );
        tokio::spawn(async move {
            match handler.await {
                Ok(()) | Err(future::Aborted) => info!("[{}] subscriber shutdown.", local_addr),
            }
        });
        Ok(SubscriberHandle(abort_handle))
    }
}

#[derive(Clone, Debug)]
struct Publisher {
    clients: Arc<Mutex<HashMap<SocketAddr, subscriber::SubscriberClient>>>,
}

struct PublisherAddrs {
    publisher: SocketAddr,
    subscriptions: SocketAddr,
}

impl Publisher {
    async fn start(self) -> io::Result<PublisherAddrs> {
        let mut connecting_publishers = tcp::listen("localhost:0", Json::default).await?;

        let publisher_addrs = PublisherAddrs {
            publisher: connecting_publishers.local_addr(),
            subscriptions: Self::start_subscription_manager(self.clients.clone()).await?,
        };

        info!("[{}] listening for publishers.", publisher_addrs.publisher);
        tokio::spawn(async move {
            // Because this is just an example, we know there will only be one publisher. In more
            // realistic code, this would be a loop to continually accept new publisher
            // connections.
            let publisher = connecting_publishers.next().await.unwrap().unwrap();
            info!("[{}] publisher connected.", publisher.peer_addr().unwrap());

            server::BaseChannel::with_defaults(publisher)
                .respond_with(self.serve())
                .execute()
                .await
        });

        Ok(publisher_addrs)
    }

    async fn start_subscription_manager(
        clients: Arc<Mutex<HashMap<SocketAddr, subscriber::SubscriberClient>>>,
    ) -> io::Result<SocketAddr> {
        let mut connecting_subscribers = tcp::listen("localhost:0", Json::default)
            .await?
            .filter_map(|r| future::ready(r.ok()));
        let new_subscriber_addr = connecting_subscribers.get_ref().local_addr();
        info!("[{}] listening for subscribers.", new_subscriber_addr);

        tokio::spawn(async move {
            while let Some(conn) = connecting_subscribers.next().await {
                let subscriber_addr = conn.peer_addr().unwrap();
                info!("[{}] subscriber connected.", subscriber_addr);

                let tarpc::client::NewClient {
                    client: subscriber,
                    dispatch,
                } = subscriber::SubscriberClient::new(client::Config::default(), conn);
                clients.lock().unwrap().insert(subscriber_addr, subscriber);

                let dropped_clients = clients.clone();
                tokio::spawn(async move {
                    match dispatch.await {
                        Ok(()) => info!("[{:?}] subscriber connection closed", subscriber_addr),
                        Err(e) => info!(
                            "[{:?}] subscriber connection broken: {:?}",
                            subscriber_addr, e
                        ),
                    }
                    dropped_clients.lock().unwrap().remove(&subscriber_addr);
                });
            }
        });

        Ok(new_subscriber_addr)
    }
}

#[tarpc::server]
impl publisher::Publisher for Publisher {
    async fn broadcast(self, _: context::Context, message: String) {
        info!("received message to broadcast.");
        let mut clients = self.clients.lock().unwrap().clone();
        let mut publications = Vec::new();
        for client in clients.values_mut() {
            publications.push(client.receive(context::current(), message.clone()));
        }
        // Ignore failing subscribers. In a real pubsub, you'd want to continually retry until
        // subscribers ack. Of course, a lot would be different in a real pubsub :)
        future::join_all(publications).await;
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    env_logger::init();

    let clients = Arc::new(Mutex::new(HashMap::new()));
    let addrs = Publisher { clients }.start().await?;

    let mut publisher = publisher::PublisherClient::new(
        client::Config::default(),
        tcp::connect(addrs.publisher, Json::default()).await?,
    )
    .spawn()?;

    let _subscriber0 = Subscriber::connect(addrs.subscriptions).await?;
    publisher
        .broadcast(context::current(), "hello to one".to_string())
        .await?;

    let _subscriber1 = Subscriber::connect(addrs.subscriptions).await?;
    publisher
        .broadcast(context::current(), "hello to all".to_string())
        .await?;

    drop(_subscriber0);

    publisher
        .broadcast(context::current(), "hello to who?".to_string())
        .await?;

    info!("done.");

    Ok(())
}
