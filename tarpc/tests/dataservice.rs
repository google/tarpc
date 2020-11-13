use futures::prelude::*;
use std::io;
use tarpc::serde_transport;
use tarpc::{client, context, server::Handler};
use tokio_serde::formats::Json;

#[tarpc::derive_serde]
#[derive(Debug, PartialEq)]
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

#[tarpc::server]
impl ColorProtocol for ColorServer {
    async fn get_opposite_color(self, _: context::Context, color: TestData) -> TestData {
        match color {
            TestData::White => TestData::Black,
            TestData::Black => TestData::White,
        }
    }
}

#[tokio::test]
async fn test_call() -> io::Result<()> {
    let transport = tarpc::serde_transport::tcp::listen("localhost:56797", Json::default).await?;
    let addr = transport.local_addr();
    tokio::spawn(
        tarpc::Server::default()
            .incoming(transport.take(1).filter_map(|r| async { r.ok() }))
            .respond_with(ColorServer.serve()),
    );

    let transport = serde_transport::tcp::connect(addr, Json::default).await?;
    let mut client = ColorProtocolClient::new(client::Config::default(), transport).spawn()?;

    let color = client
        .get_opposite_color(context::current(), TestData::White)
        .await?;
    assert_eq!(color, TestData::Black);

    Ok(())
}
