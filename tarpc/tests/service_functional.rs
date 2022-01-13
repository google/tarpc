use assert_matches::assert_matches;
use futures::{
    future::{join_all, ready, Ready},
    prelude::*,
};
use std::time::{Duration, SystemTime};
use tarpc::{
    client::{self},
    context,
    server::{self, incoming::Incoming, BaseChannel, Channel},
    transport::channel,
};
use tokio::join;

#[tarpc_plugins::service]
trait Service {
    async fn add(x: i32, y: i32) -> i32;
    async fn hey(name: String) -> String;
}

#[derive(Clone)]
struct Server;

impl Service for Server {
    type AddFut = Ready<i32>;

    fn add(self, _: context::Context, x: i32, y: i32) -> Self::AddFut {
        ready(x + y)
    }

    type HeyFut = Ready<String>;

    fn hey(self, _: context::Context, name: String) -> Self::HeyFut {
        ready(format!("Hey, {name}."))
    }
}

#[tokio::test]
async fn sequential() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    let (tx, rx) = channel::unbounded();

    tokio::spawn(
        BaseChannel::new(server::Config::default(), rx)
            .requests()
            .execute(Server.serve()),
    );

    let client = ServiceClient::new(client::Config::default(), tx).spawn();

    assert_matches!(client.add(context::current(), 1, 2).await, Ok(3));
    assert_matches!(
        client.hey(context::current(), "Tim".into()).await,
        Ok(ref s) if s == "Hey, Tim.");

    Ok(())
}

#[tokio::test]
async fn dropped_channel_aborts_in_flight_requests() -> anyhow::Result<()> {
    #[tarpc_plugins::service]
    trait Loop {
        async fn r#loop();
    }

    #[derive(Clone)]
    struct LoopServer;

    #[derive(Debug)]
    struct AllHandlersComplete;

    #[tarpc::server]
    impl Loop for LoopServer {
        async fn r#loop(self, _: context::Context) {
            loop {
                futures::pending!();
            }
        }
    }

    let _ = tracing_subscriber::fmt::try_init();

    let (tx, rx) = channel::unbounded();

    // Set up a client that initiates a long-lived request.
    // The request will complete in error when the server drops the connection.
    tokio::spawn(async move {
        let client = LoopClient::new(client::Config::default(), tx).spawn();

        let mut ctx = context::current();
        ctx.deadline = SystemTime::now() + Duration::from_secs(60 * 60);
        let _ = client.r#loop(ctx).await;
    });

    let mut requests = BaseChannel::with_defaults(rx).requests();
    // Reading a request should trigger the request being registered with BaseChannel.
    let first_request = requests.next().await.unwrap()?;
    // Dropping the channel should trigger cleanup of outstanding requests.
    drop(requests);
    // In-flight requests should be aborted by channel cleanup.
    // The first and only request sent by the client is `loop`, which is an infinite loop
    // on the server side, so if cleanup was not triggered, this line should hang indefinitely.
    first_request.execute(LoopServer.serve()).await;

    Ok(())
}

#[cfg(all(feature = "serde-transport", feature = "tcp"))]
#[tokio::test]
async fn serde() -> anyhow::Result<()> {
    use tarpc::serde_transport;
    use tokio_serde::formats::Json;

    let _ = tracing_subscriber::fmt::try_init();

    let transport = tarpc::serde_transport::tcp::listen("localhost:56789", Json::default).await?;
    let addr = transport.local_addr();
    tokio::spawn(
        transport
            .take(1)
            .filter_map(|r| async { r.ok() })
            .map(BaseChannel::with_defaults)
            .execute(Server.serve()),
    );

    let transport = serde_transport::tcp::connect(addr, Json::default).await?;
    let client = ServiceClient::new(client::Config::default(), transport).spawn();

    assert_matches!(client.add(context::current(), 1, 2).await, Ok(3));
    assert_matches!(
        client.hey(context::current(), "Tim".to_string()).await,
        Ok(ref s) if s == "Hey, Tim."
    );

    Ok(())
}

#[tokio::test]
async fn concurrent() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    let (tx, rx) = channel::unbounded();
    tokio::spawn(
        stream::once(ready(rx))
            .map(BaseChannel::with_defaults)
            .execute(Server.serve()),
    );

    let client = ServiceClient::new(client::Config::default(), tx).spawn();

    let req1 = client.add(context::current(), 1, 2);
    let req2 = client.add(context::current(), 3, 4);
    let req3 = client.hey(context::current(), "Tim".to_string());

    assert_matches!(req1.await, Ok(3));
    assert_matches!(req2.await, Ok(7));
    assert_matches!(req3.await, Ok(ref s) if s == "Hey, Tim.");

    Ok(())
}

#[tokio::test]
async fn concurrent_join() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    let (tx, rx) = channel::unbounded();
    tokio::spawn(
        stream::once(ready(rx))
            .map(BaseChannel::with_defaults)
            .execute(Server.serve()),
    );

    let client = ServiceClient::new(client::Config::default(), tx).spawn();

    let req1 = client.add(context::current(), 1, 2);
    let req2 = client.add(context::current(), 3, 4);
    let req3 = client.hey(context::current(), "Tim".to_string());

    let (resp1, resp2, resp3) = join!(req1, req2, req3);
    assert_matches!(resp1, Ok(3));
    assert_matches!(resp2, Ok(7));
    assert_matches!(resp3, Ok(ref s) if s == "Hey, Tim.");

    Ok(())
}

#[tokio::test]
async fn concurrent_join_all() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    let (tx, rx) = channel::unbounded();
    tokio::spawn(
        stream::once(ready(rx))
            .map(BaseChannel::with_defaults)
            .execute(Server.serve()),
    );

    let client = ServiceClient::new(client::Config::default(), tx).spawn();

    let req1 = client.add(context::current(), 1, 2);
    let req2 = client.add(context::current(), 3, 4);

    let responses = join_all(vec![req1, req2]).await;
    assert_matches!(responses[0], Ok(3));
    assert_matches!(responses[1], Ok(7));

    Ok(())
}

#[tokio::test]
async fn counter() -> anyhow::Result<()> {
    #[tarpc::service]
    trait Counter {
        async fn count() -> u32;
    }

    struct CountService(u32);

    impl Counter for &mut CountService {
        type CountFut = futures::future::Ready<u32>;

        fn count(self, _: context::Context) -> Self::CountFut {
            self.0 += 1;
            futures::future::ready(self.0)
        }
    }

    let (tx, rx) = channel::unbounded();
    tokio::spawn(async {
        let mut requests = BaseChannel::with_defaults(rx).requests();
        let mut counter = CountService(0);

        while let Some(Ok(request)) = requests.next().await {
            request.execute(counter.serve()).await;
        }
    });

    let client = CounterClient::new(client::Config::default(), tx).spawn();
    assert_matches!(client.count(context::current()).await, Ok(1));
    assert_matches!(client.count(context::current()).await, Ok(2));

    Ok(())
}
