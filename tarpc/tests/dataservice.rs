use futures::prelude::*;
use tarpc::serde_transport;
use tarpc::{
    client, context,
    server::{incoming::Incoming, BaseChannel},
};
use tokio_serde::formats::Json;

#[tarpc::derive_serde]
#[derive(Debug, PartialEq, Eq)]
pub enum TestData {
    Black,
    White,
}

#[tarpc::service]
pub trait ColorProtocol {
    async fn get_opposite_color(color: TestData) -> TestData;
}

#[derive(Clone)]
struct ColorServer;

impl ColorProtocol for ColorServer {
    async fn get_opposite_color(self, _: context::Context, color: TestData) -> TestData {
        match color {
            TestData::White => TestData::Black,
            TestData::Black => TestData::White,
        }
    }
}

#[cfg(test)]
async fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
    tokio::spawn(fut);
}

#[tokio::test]
async fn test_call() -> anyhow::Result<()> {
    let transport = tarpc::serde_transport::tcp::listen("localhost:56797", Json::default).await?;
    let addr = transport.local_addr();
    tokio::spawn(
        transport
            .take(1)
            .filter_map(|r| async { r.ok() })
            .map(BaseChannel::with_defaults)
            .execute(ColorServer.serve())
            .map(|channel| channel.for_each(spawn))
            .for_each(spawn),
    );

    let transport = serde_transport::tcp::connect(addr, Json::default).await?;
    let client = ColorProtocolClient::new(client::Config::default(), transport).spawn();

    let color = client
        .get_opposite_color(context::current(), TestData::White)
        .await?;
    assert_eq!(color, TestData::Black);

    Ok(())
}
