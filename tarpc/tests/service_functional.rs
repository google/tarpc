use assert_matches::assert_matches;
use futures::{
    future::{join_all, ready, Ready},
    prelude::*,
};
use std::{
    io,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tarpc::{
    client::{self},
    context,
    server::{self, BaseChannel, Channel, Handler},
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
        ready(format!("Hey, {}.", name))
    }
}

#[tokio::test]
async fn sequential() -> io::Result<()> {
    let _ = env_logger::try_init();

    let (tx, rx) = channel::unbounded();

    tokio::spawn(
        BaseChannel::new(server::Config::default(), rx)
            .respond_with(Server.serve())
            .execute(),
    );

    let mut client = ServiceClient::new(client::Config::default(), tx).spawn()?;

    assert_matches!(client.add(context::current(), 1, 2).await, Ok(3));
    assert_matches!(
        client.hey(context::current(), "Tim".into()).await,
        Ok(ref s) if s == "Hey, Tim.");

    Ok(())
}

#[tokio::test]
async fn dropped_channel_aborts_in_flight_requests() -> io::Result<()> {
    #[tarpc_plugins::service]
    trait Loop {
        async fn r#loop();
    }

    struct LoopServer(tokio::sync::mpsc::UnboundedSender<AllHandlersComplete>);

    #[derive(Debug)]
    struct AllHandlersComplete;

    impl Drop for LoopServer {
        fn drop(&mut self) {
            let _ = self.0.send(AllHandlersComplete);
        }
    }

    #[tarpc::server]
    impl Loop for Arc<LoopServer> {
        async fn r#loop(self, _: context::Context) {
            loop {
                futures::pending!();
            }
        }
    }

    let _ = env_logger::try_init();

    let (tx, rx) = channel::unbounded();
    let (rpc_finished_tx, mut rpc_finished) = tokio::sync::mpsc::unbounded_channel();

    // Set up a client that initiates a long-lived request.
    // The request will complete in error when the server drops the connection.
    tokio::spawn(async move {
        let mut client = LoopClient::new(client::Config::default(), tx)
            .spawn()
            .unwrap();

        let mut ctx = context::current();
        ctx.deadline = SystemTime::now() + Duration::from_secs(60 * 60);
        let _ = client.r#loop(ctx).await;
    });

    let mut server =
        BaseChannel::with_defaults(rx).respond_with(Arc::new(LoopServer(rpc_finished_tx)).serve());
    let first_handler = server.next().await.unwrap()?;

    drop(server);
    first_handler.await;

    // At this point, a single RPC has been sent and a single response initiated.
    // The request handler will loop for a long time unless aborted.
    // Now, we assert that the act of disconnecting a client is sufficient to abort all
    // handlers initiated by the connection's RPCs.
    assert_matches!(rpc_finished.recv().await, Some(AllHandlersComplete));
    Ok(())
}

#[cfg(all(feature = "serde-transport", feature = "tcp"))]
#[tokio::test]
async fn serde() -> io::Result<()> {
    use tarpc::serde_transport;
    use tokio_serde::formats::Json;

    let _ = env_logger::try_init();

    let transport = tarpc::serde_transport::tcp::listen("localhost:56789", Json::default).await?;
    let addr = transport.local_addr();
    tokio::spawn(
        tarpc::Server::default()
            .incoming(transport.take(1).filter_map(|r| async { r.ok() }))
            .respond_with(Server.serve()),
    );

    let transport = serde_transport::tcp::connect(addr, Json::default).await?;
    let mut client = ServiceClient::new(client::Config::default(), transport).spawn()?;

    assert_matches!(client.add(context::current(), 1, 2).await, Ok(3));
    assert_matches!(
        client.hey(context::current(), "Tim".to_string()).await,
        Ok(ref s) if s == "Hey, Tim."
    );

    Ok(())
}

#[tokio::test]
async fn concurrent() -> io::Result<()> {
    let _ = env_logger::try_init();

    let (tx, rx) = channel::unbounded();
    tokio::spawn(
        tarpc::Server::default()
            .incoming(stream::once(ready(rx)))
            .respond_with(Server.serve()),
    );

    let client = ServiceClient::new(client::Config::default(), tx).spawn()?;

    let mut c = client.clone();
    let req1 = c.add(context::current(), 1, 2);

    let mut c = client.clone();
    let req2 = c.add(context::current(), 3, 4);

    let mut c = client.clone();
    let req3 = c.hey(context::current(), "Tim".to_string());

    assert_matches!(req1.await, Ok(3));
    assert_matches!(req2.await, Ok(7));
    assert_matches!(req3.await, Ok(ref s) if s == "Hey, Tim.");

    Ok(())
}

#[tokio::test]
async fn concurrent_join() -> io::Result<()> {
    let _ = env_logger::try_init();

    let (tx, rx) = channel::unbounded();
    tokio::spawn(
        tarpc::Server::default()
            .incoming(stream::once(ready(rx)))
            .respond_with(Server.serve()),
    );

    let client = ServiceClient::new(client::Config::default(), tx).spawn()?;

    let mut c = client.clone();
    let req1 = c.add(context::current(), 1, 2);

    let mut c = client.clone();
    let req2 = c.add(context::current(), 3, 4);

    let mut c = client.clone();
    let req3 = c.hey(context::current(), "Tim".to_string());

    let (resp1, resp2, resp3) = join!(req1, req2, req3);
    assert_matches!(resp1, Ok(3));
    assert_matches!(resp2, Ok(7));
    assert_matches!(resp3, Ok(ref s) if s == "Hey, Tim.");

    Ok(())
}

#[tokio::test]
async fn concurrent_join_all() -> io::Result<()> {
    let _ = env_logger::try_init();

    let (tx, rx) = channel::unbounded();
    tokio::spawn(
        tarpc::Server::default()
            .incoming(stream::once(ready(rx)))
            .respond_with(Server.serve()),
    );

    let client = ServiceClient::new(client::Config::default(), tx).spawn()?;

    let mut c1 = client.clone();
    let mut c2 = client.clone();

    let req1 = c1.add(context::current(), 1, 2);
    let req2 = c2.add(context::current(), 3, 4);

    let responses = join_all(vec![req1, req2]).await;
    assert_matches!(responses[0], Ok(3));
    assert_matches!(responses[1], Ok(7));

    Ok(())
}
