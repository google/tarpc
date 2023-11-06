// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

/// - The PubSub server sets up TCP listeners on 2 ports, the "subscriber" port and the "publisher"
///   port. Because both publishers and subscribers initiate their connections to the PubSub
///   server, the server requires no prior knowledge of either publishers or subscribers.
///
/// - Subscribers connect to the server on the server's "subscriber" port. Once a connection is
///   established, the server acts as the client of the Subscriber service, initially requesting
///   the topics the subscriber is interested in, and subsequently sending topical messages to the
///   subscriber.
///
/// - Publishers connect to the server on the "publisher" port and, once connected, they send
///   topical messages via Publisher service to the server. The server then broadcasts each
///   messages to all clients subscribed to the topic of that message.
///
///       Subscriber                        Publisher                       PubSub Server
/// T1        |                                 |                                 |
/// T2        |-----Connect------------------------------------------------------>|
/// T3        |                                 |                                 |
/// T2        |<-------------------------------------------------------Topics-----|
/// T2        |-----(OK) Topics-------------------------------------------------->|
/// T3        |                                 |                                 |
/// T4        |                                 |-----Connect-------------------->|
/// T5        |                                 |                                 |
/// T6        |                                 |-----Publish-------------------->|
/// T7        |                                 |                                 |
/// T8        |<------------------------------------------------------Receive-----|
/// T9        |-----(OK) Receive------------------------------------------------->|
/// T10       |                                 |                                 |
/// T11       |                                 |<--------------(OK) Publish------|
use anyhow::anyhow;
use futures::{
    channel::oneshot,
    future::{self, AbortHandle},
    prelude::*,
};
use publisher::Publisher as _;
use std::{
    collections::HashMap,
    env,
    error::Error,
    io,
    net::SocketAddr,
    sync::{Arc, Mutex, RwLock},
};
use subscriber::Subscriber as _;
use tarpc::{
    client, context,
    serde_transport::tcp,
    server::{self, Channel},
    tokio_serde::formats::Json,
};
use tokio::net::ToSocketAddrs;
use tracing::info;
use tracing_subscriber::prelude::*;

pub mod subscriber {
    #[tarpc::service]
    pub trait Subscriber {
        async fn topics() -> Vec<String>;
        async fn receive(topic: String, message: String);
    }
}

pub mod publisher {
    #[tarpc::service]
    pub trait Publisher {
        async fn publish(topic: String, message: String);
    }
}

#[derive(Clone, Debug)]
struct Subscriber {
    local_addr: SocketAddr,
    topics: Vec<String>,
}

impl subscriber::Subscriber for Subscriber {
    async fn topics(self, _: context::Context) -> Vec<String> {
        self.topics.clone()
    }

    async fn receive(self, _: context::Context, topic: String, message: String) {
        info!(local_addr = %self.local_addr, %topic, %message, "ReceivedMessage")
    }
}

struct SubscriberHandle(AbortHandle);

impl Drop for SubscriberHandle {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Subscriber {
    async fn connect(
        publisher_addr: impl ToSocketAddrs,
        topics: Vec<String>,
    ) -> anyhow::Result<SubscriberHandle> {
        let publisher = tcp::connect(publisher_addr, Json::default).await?;
        let local_addr = publisher.local_addr()?;
        let mut handler = server::BaseChannel::with_defaults(publisher).requests();
        let subscriber = Subscriber { local_addr, topics };
        // The first request is for the topics being subscribed to.
        match handler.next().await {
            Some(init_topics) => init_topics?.execute(subscriber.clone().serve()).await,
            None => {
                return Err(anyhow!(
                    "[{}] Server never initialized the subscriber.",
                    local_addr
                ))
            }
        };
        let (handler, abort_handle) =
            future::abortable(handler.execute(subscriber.serve()).for_each(spawn));
        tokio::spawn(async move {
            match handler.await {
                Ok(()) | Err(future::Aborted) => info!(?local_addr, "subscriber shutdown."),
            }
        });
        Ok(SubscriberHandle(abort_handle))
    }
}

#[derive(Debug)]
struct Subscription {
    topics: Vec<String>,
}

#[derive(Clone, Debug)]
struct Publisher {
    clients: Arc<Mutex<HashMap<SocketAddr, Subscription>>>,
    subscriptions: Arc<RwLock<HashMap<String, HashMap<SocketAddr, subscriber::SubscriberClient>>>>,
}

struct PublisherAddrs {
    publisher: SocketAddr,
    subscriptions: SocketAddr,
}

async fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
    tokio::spawn(fut);
}

impl Publisher {
    async fn start(self) -> io::Result<PublisherAddrs> {
        let mut connecting_publishers = tcp::listen("localhost:0", Json::default).await?;

        let publisher_addrs = PublisherAddrs {
            publisher: connecting_publishers.local_addr(),
            subscriptions: self.clone().start_subscription_manager().await?,
        };

        info!(publisher_addr = %publisher_addrs.publisher, "listening for publishers.",);
        tokio::spawn(async move {
            // Because this is just an example, we know there will only be one publisher. In more
            // realistic code, this would be a loop to continually accept new publisher
            // connections.
            let publisher = connecting_publishers.next().await.unwrap().unwrap();
            info!(publisher.peer_addr = ?publisher.peer_addr(), "publisher connected.");

            server::BaseChannel::with_defaults(publisher)
                .execute(self.serve())
                .for_each(spawn)
                .await
        });

        Ok(publisher_addrs)
    }

    async fn start_subscription_manager(mut self) -> io::Result<SocketAddr> {
        let mut connecting_subscribers = tcp::listen("localhost:0", Json::default)
            .await?
            .filter_map(|r| future::ready(r.ok()));
        let new_subscriber_addr = connecting_subscribers.get_ref().local_addr();
        info!(?new_subscriber_addr, "listening for subscribers.");

        tokio::spawn(async move {
            while let Some(conn) = connecting_subscribers.next().await {
                let subscriber_addr = conn.peer_addr().unwrap();

                let tarpc::client::NewClient {
                    client: subscriber,
                    dispatch,
                } = subscriber::SubscriberClient::new(client::Config::default(), conn);
                let (ready_tx, ready) = oneshot::channel();
                self.clone()
                    .start_subscriber_gc(subscriber_addr, dispatch, ready);

                // Populate the topics
                self.initialize_subscription(subscriber_addr, subscriber)
                    .await;

                // Signal that initialization is done.
                ready_tx.send(()).unwrap();
            }
        });

        Ok(new_subscriber_addr)
    }

    async fn initialize_subscription(
        &mut self,
        subscriber_addr: SocketAddr,
        subscriber: subscriber::SubscriberClient,
    ) {
        // Populate the topics
        if let Ok(topics) = subscriber.topics(context::current()).await {
            self.clients.lock().unwrap().insert(
                subscriber_addr,
                Subscription {
                    topics: topics.clone(),
                },
            );

            info!(%subscriber_addr, ?topics, "subscribed to new topics");
            let mut subscriptions = self.subscriptions.write().unwrap();
            for topic in topics {
                subscriptions
                    .entry(topic)
                    .or_insert_with(HashMap::new)
                    .insert(subscriber_addr, subscriber.clone());
            }
        }
    }

    fn start_subscriber_gc<E: Error>(
        self,
        subscriber_addr: SocketAddr,
        client_dispatch: impl Future<Output = Result<(), E>> + Send + 'static,
        subscriber_ready: oneshot::Receiver<()>,
    ) {
        tokio::spawn(async move {
            if let Err(e) = client_dispatch.await {
                info!(
                    %subscriber_addr,
                    error = %e,
                    "subscriber connection broken");
            }
            // Don't clean up the subscriber until initialization is done.
            let _ = subscriber_ready.await;
            if let Some(subscription) = self.clients.lock().unwrap().remove(&subscriber_addr) {
                info!(
                    "[{} unsubscribing from topics: {:?}",
                    subscriber_addr, subscription.topics
                );
                let mut subscriptions = self.subscriptions.write().unwrap();
                for topic in subscription.topics {
                    let subscribers = subscriptions.get_mut(&topic).unwrap();
                    subscribers.remove(&subscriber_addr);
                    if subscribers.is_empty() {
                        subscriptions.remove(&topic);
                    }
                }
            }
        });
    }
}

impl publisher::Publisher for Publisher {
    async fn publish(self, _: context::Context, topic: String, message: String) {
        info!("received message to publish.");
        let mut subscribers = match self.subscriptions.read().unwrap().get(&topic) {
            None => return,
            Some(subscriptions) => subscriptions.clone(),
        };
        let mut publications = Vec::new();
        for client in subscribers.values_mut() {
            publications.push(client.receive(context::current(), topic.clone(), message.clone()));
        }
        // Ignore failing subscribers. In a real pubsub, you'd want to continually retry until
        // subscribers ack. Of course, a lot would be different in a real pubsub :)
        for response in future::join_all(publications).await {
            if let Err(e) = response {
                info!("failed to broadcast to subscriber: {}", e);
            }
        }
    }
}

/// Initializes an OpenTelemetry tracing subscriber with a Jaeger backend.
fn init_tracing(service_name: &str) -> anyhow::Result<()> {
    env::set_var("OTEL_BSP_MAX_EXPORT_BATCH_SIZE", "12");
    let tracer = opentelemetry_jaeger::new_agent_pipeline()
        .with_service_name(service_name)
        .with_max_packet_size(2usize.pow(13))
        .install_batch(opentelemetry::runtime::Tokio)?;

    tracing_subscriber::registry()
        .with(tracing_subscriber::filter::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .try_init()?;

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing("Pub/Sub")?;

    let addrs = Publisher {
        clients: Arc::new(Mutex::new(HashMap::new())),
        subscriptions: Arc::new(RwLock::new(HashMap::new())),
    }
    .start()
    .await?;

    let _subscriber0 = Subscriber::connect(
        addrs.subscriptions,
        vec!["calculus".into(), "cool shorts".into()],
    )
    .await?;

    let _subscriber1 = Subscriber::connect(
        addrs.subscriptions,
        vec!["cool shorts".into(), "history".into()],
    )
    .await?;

    let publisher = publisher::PublisherClient::new(
        client::Config::default(),
        tcp::connect(addrs.publisher, Json::default).await?,
    )
    .spawn();

    publisher
        .publish(context::current(), "calculus".into(), "sqrt(2)".into())
        .await?;

    publisher
        .publish(
            context::current(),
            "cool shorts".into(),
            "hello to all".into(),
        )
        .await?;

    publisher
        .publish(context::current(), "history".into(), "napoleon".to_string())
        .await?;

    drop(_subscriber0);

    publisher
        .publish(
            context::current(),
            "cool shorts".into(),
            "hello to who?".into(),
        )
        .await?;

    opentelemetry::global::shutdown_tracer_provider();
    info!("done.");

    Ok(())
}
