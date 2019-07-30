#![feature(async_await)]

use assert_matches::assert_matches;
use futures::{
    future::{ready, Ready},
    prelude::*,
};
#[cfg(feature = "serde1")]
use std::io;
use tarpc::{client, context, server::Handler, transport::channel};

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

#[runtime::test(runtime_tokio::TokioCurrentThread)]
async fn sequential() {
    let _ = env_logger::try_init();

    let (tx, rx) = channel::unbounded();
    let _ = runtime::spawn(
        tarpc::Server::default()
            .incoming(stream::once(ready(rx)))
            .respond_with(serve(Server)),
    );

    let mut client = new_stub(client::Config::default(), tx).await.unwrap();
    assert_eq!(3, client.add(context::current(), 1, 2).await.unwrap());
    assert_eq!(
        "Hey, Tim.",
        client
            .hey(context::current(), "Tim".to_string())
            .await
            .unwrap()
    );
}

#[cfg(feature = "serde1")]
#[runtime::test(runtime_tokio::TokioCurrentThread)]
async fn serde() -> io::Result<()> {
    let _ = env_logger::try_init();

    let transport = bincode_transport::listen(&([0, 0, 0, 0], 56789).into())?;
    let addr = transport.local_addr();
    let _ = runtime::spawn(
        tarpc::Server::default()
            .incoming(transport.take(1).filter_map(|r| async { r.ok() }))
            .respond_with(serve(Server)),
    );

    let transport = bincode_transport::connect(&addr).await?;
    let mut client = new_stub(client::Config::default(), transport).await?;

    assert_matches!(client.add(context::current(), 1, 2).await, Ok(3));
    assert_matches!(
        client.hey(context::current(), "Tim".to_string()).await,
        Ok(ref s) if s == "Hey, Tim."
    );

    Ok(())
}

#[runtime::test(runtime_tokio::TokioCurrentThread)]
async fn concurrent() {
    let _ = env_logger::try_init();

    let (tx, rx) = channel::unbounded();
    let _ = runtime::spawn(
        rpc::Server::default()
            .incoming(stream::once(ready(rx)))
            .respond_with(serve(Server)),
    );

    let client = new_stub(client::Config::default(), tx).await.unwrap();
    let mut c = client.clone();
    let req1 = c.add(context::current(), 1, 2);
    let mut c = client.clone();
    let req2 = c.add(context::current(), 3, 4);
    let mut c = client.clone();
    let req3 = c.hey(context::current(), "Tim".to_string());

    assert_eq!(3, req1.await.unwrap());
    assert_eq!(7, req2.await.unwrap());
    assert_eq!("Hey, Tim.", req3.await.unwrap());
}
