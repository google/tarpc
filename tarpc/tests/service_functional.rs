use assert_matches::assert_matches;
use futures::{
    future::{ready, Ready},
    prelude::*,
};
use std::io;
use tarpc::{
    client::{self},
    context, json_transport,
    server::{self, BaseChannel, Channel, Handler},
    transport::channel,
};

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

#[tokio::test(threaded_scheduler)]
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

#[cfg(feature = "serde1")]
#[tokio::test(threaded_scheduler)]
async fn serde() -> io::Result<()> {
    let _ = env_logger::try_init();

    let transport = json_transport::listen("localhost:56789").await?;
    let addr = transport.local_addr();
    tokio::spawn(
        tarpc::Server::default()
            .incoming(transport.take(1).filter_map(|r| async { r.ok() }))
            .respond_with(Server.serve()),
    );

    let transport = json_transport::connect(addr).await?;
    let mut client = ServiceClient::new(client::Config::default(), transport).spawn()?;

    assert_matches!(client.add(context::current(), 1, 2).await, Ok(3));
    assert_matches!(
        client.hey(context::current(), "Tim".to_string()).await,
        Ok(ref s) if s == "Hey, Tim."
    );

    Ok(())
}

#[tokio::test(threaded_scheduler)]
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
