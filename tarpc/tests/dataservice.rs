use futures::{future, prelude::*};
use std::io;
use tarpc::{
    client, context,
    server::{self, Handler},
};

#[tarpc::derive_serde]
#[derive(PartialEq)]
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
    let (client_transport, server_transport) = tarpc::transport::channel::unbounded();

    let server = server::new(server::Config::default())
        .incoming(stream::once(future::ready(server_transport)))
        .respond_with(ColorServer.serve());

    tokio::spawn(server);

    let mut client =
        ColorProtocolClient::new(client::Config::default(), client_transport).spawn()?;

    let color = client
        .get_opposite_color(context::current(), TestData::White)
        .await?;
    assert_eq!(color, TestData::Black);

    Ok(())
}
