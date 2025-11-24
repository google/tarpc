// Copyright 2022 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use futures::prelude::*;
use tarpc::context::{ClientContext, ServerContext, SharedContext};
use tarpc::server::{BaseChannel, Channel};
use tarpc::tokio_serde::formats::Bincode;
use tarpc::tokio_util::codec::length_delimited::LengthDelimitedCodec;
use tarpc::transport::channel::{map_client_context_to_shared, map_shared_context_to_server};
use tarpc::{ClientMessage, serde_transport as transport};
use tokio::net::{UnixListener, UnixStream};

#[tarpc::service]
pub trait PingService {
    async fn ping();
}

#[derive(Clone)]
struct Service;

impl PingService for Service {
    async fn ping(self, _: &mut ServerContext) {}
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bind_addr = "/tmp/tarpc_on_unix_example.sock";

    let _ = std::fs::remove_file(bind_addr);

    let listener = UnixListener::bind(bind_addr).unwrap();
    let codec_builder = LengthDelimitedCodec::builder();
    async fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        tokio::spawn(fut);
    }
    tokio::spawn(async move {
        loop {
            let (conn, _addr) = listener.accept().await.unwrap();
            let framed = codec_builder.new_framed(conn);
            let transport = transport::new(framed, Bincode::default());
            let transport = transport.map_ok(map_shared_context_to_server);

            let fut = BaseChannel::with_defaults(transport)
                .execute(Service.serve())
                .for_each(spawn);
            tokio::spawn(fut);
        }
    });

    let conn = UnixStream::connect(bind_addr).await?;
    let transport = transport::new(codec_builder.new_framed(conn), Bincode::default());
    let transport = transport.with(|msg| future::ok(map_client_context_to_shared(msg)));
    PingServiceClient::new(Default::default(), transport)
        .spawn()
        .ping(&mut ClientContext::current())
        .await?;

    Ok(())
}
