use assert_matches::assert_matches;
use futures::{
    future::{join_all, ready},
    prelude::*,
};
use std::time::{Duration, SystemTime};
use tarpc::{
    client::{self},
    context,
    server::{incoming::Incoming, BaseChannel, Channel},
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
    async fn add(self, _: context::Context, x: i32, y: i32) -> i32 {
        x + y
    }

    async fn hey(self, _: context::Context, name: String) -> String {
        format!("Hey, {name}.")
    }
}

#[tokio::test]
async fn sequential() {
    let (tx, rx) = tarpc::transport::channel::unbounded();
    let client = client::new(client::Config::default(), tx).spawn();
    let channel = BaseChannel::with_defaults(rx);
    tokio::spawn(
        channel
            .execute(tarpc::server::serve(|_, i| async move { Ok(i + 1) }))
            .for_each(|response| response),
    );
    assert_eq!(
        client.call(context::current(), "AddOne", 1).await.unwrap(),
        2
    );
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
async fn serde_tcp() -> anyhow::Result<()> {
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
            .execute(Server.serve())
            .map(|channel| channel.for_each(spawn))
            .for_each(spawn),
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

#[cfg(all(feature = "serde-transport", feature = "unix", unix))]
#[tokio::test]
async fn serde_uds() -> anyhow::Result<()> {
    use tarpc::serde_transport;
    use tokio_serde::formats::Json;

    let _ = tracing_subscriber::fmt::try_init();

    let sock = tarpc::serde_transport::unix::TempPathBuf::with_random("uds");
    let transport = tarpc::serde_transport::unix::listen(&sock, Json::default).await?;
    tokio::spawn(
        transport
            .take(1)
            .filter_map(|r| async { r.ok() })
            .map(BaseChannel::with_defaults)
            .execute(Server.serve())
            .map(|channel| channel.for_each(spawn))
            .for_each(spawn),
    );

    let transport = serde_transport::unix::connect(&sock, Json::default).await?;
    let client = ServiceClient::new(client::Config::default(), transport).spawn();

    // Save results using socket so we can clean the socket even if our test assertions fail
    let res1 = client.add(context::current(), 1, 2).await;
    let res2 = client.hey(context::current(), "Tim".to_string()).await;

    assert_matches!(res1, Ok(3));
    assert_matches!(res2, Ok(ref s) if s == "Hey, Tim.");

    Ok(())
}

#[tokio::test]
async fn concurrent() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    let (tx, rx) = channel::unbounded();
    tokio::spawn(
        stream::once(ready(rx))
            .map(BaseChannel::with_defaults)
            .execute(Server.serve())
            .map(|channel| channel.for_each(spawn))
            .for_each(spawn),
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
            .execute(Server.serve())
            .map(|channel| channel.for_each(spawn))
            .for_each(spawn),
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

#[cfg(test)]
async fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
    tokio::spawn(fut);
}

#[tokio::test]
async fn concurrent_join_all() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    let (tx, rx) = channel::unbounded();
    tokio::spawn(
        BaseChannel::with_defaults(rx)
            .execute(Server.serve())
            .for_each(spawn),
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
        async fn count(self, _: context::Context) -> u32 {
            self.0 += 1;
            self.0
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
