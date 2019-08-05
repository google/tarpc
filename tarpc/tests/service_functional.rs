#![feature(async_await)]


use assert_matches::assert_matches;
use futures::{
    future::{ready, Ready},
    prelude::*,
};
use std::{rc::Rc, io};
use tarpc::{
    client::{self, NewClient}, context,
    server::{self, BaseChannel, Channel, Handler},
    transport::channel,
};

trait RuntimeExt {
    fn exec_bg(&mut self, future: impl Future<Output = ()> + 'static);
    fn exec<F, T, E>(&mut self, future: F) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>;
}

impl RuntimeExt for tokio::runtime::current_thread::Runtime {
    fn exec_bg(&mut self, future: impl Future<Output = ()> + 'static) {
        self.spawn(Box::pin(future.unit_error()).compat());
    }

    fn exec<F, T, E>(&mut self, future: F) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
    {
        self.block_on(futures::compat::Compat::new(Box::pin(future)))
    }
}

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
async fn sequential() -> io::Result<()> {
    let _ = env_logger::try_init();

    let (tx, rx) = channel::unbounded();

    let _ = runtime::spawn(
        BaseChannel::new(server::Config::default(), rx)
            .respond_with(Server.serve())
            .execute()
    );

    let mut client = ServiceClient::new(client::Config::default(), tx).spawn()?;

    assert_matches!(client.add(context::current(), 1, 2).await, Ok(3));
    assert_matches!(
        client.hey(context::current(), "Tim".into()).await,
        Ok(ref s) if s == "Hey, Tim.");

    Ok(())
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
            .respond_with(Server.serve()),
    );

    let transport = bincode_transport::connect(&addr).await?;
    let mut client = ServiceClient::new(client::Config::default(), transport).spawn()?;

    assert_matches!(client.add(context::current(), 1, 2).await, Ok(3));
    assert_matches!(
        client.hey(context::current(), "Tim".to_string()).await,
        Ok(ref s) if s == "Hey, Tim."
    );

    Ok(())
}

#[runtime::test(runtime_tokio::TokioCurrentThread)]
async fn concurrent() -> io::Result<()> {
    let _ = env_logger::try_init();

    let (tx, rx) = channel::unbounded();
    let _ = runtime::spawn(
        rpc::Server::default()
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

#[tarpc::service(derive_serde = false)]
trait InMemory {
    async fn strong_count(rc: Rc<()>) -> usize;
    async fn weak_count(rc: Rc<()>) -> usize;
}

impl InMemory for () {
    type StrongCountFut = Ready<usize>;
    fn strong_count(self, _: context::Context, rc: Rc<()>) -> Self::StrongCountFut {
        ready(Rc::strong_count(&rc))
    }

    type WeakCountFut = Ready<usize>;
    fn weak_count(self, _: context::Context, rc: Rc<()>) -> Self::WeakCountFut {
        ready(Rc::weak_count(&rc))
    }
}

#[test]
fn in_memory_single_threaded() -> io::Result<()> {
    use log::warn;

    let _ = env_logger::try_init();
    let mut runtime = tokio::runtime::current_thread::Runtime::new()?;

    let (tx, rx) = channel::unbounded();

    let server = BaseChannel::new(server::Config::default(), rx)
        .respond_with(().serve())
        .try_for_each(|r| async move { Ok(r.await) });
    runtime.exec_bg(async {
        if let Err(e) = server.await {
            warn!("Error while running server: {}", e);
        }
    });

    let NewClient{mut client, dispatch} = InMemoryClient::new(client::Config::default(), tx);
    runtime.exec_bg(async move {
        if let Err(e) = dispatch.await {
            warn!("Error while running client dispatch: {}", e)
        }
    });

    let rc = Rc::new(());
    assert_matches!(
        runtime.exec(client.strong_count(context::current(), rc.clone())),
        Ok(2)
    );

    let _weak = Rc::downgrade(&rc);
    assert_matches!(
        runtime.exec(client.weak_count(context::current(), rc)),
        Ok(1)
    );

    Ok(())
}
