use futures::future;
use tarpc::context::Context;
use tarpc::serde_transport as transport;
use tarpc::server::{BaseChannel, Channel};
use tokio::net::{UnixListener, UnixStream};
use tokio_serde::formats::Bincode;
use tokio_util::codec::length_delimited::LengthDelimitedCodec;

#[tarpc::service]
pub trait PingService {
    async fn ping();
}

#[derive(Clone)]
struct Service;

impl PingService for Service {
    type PingFut = future::Ready<()>;

    fn ping(self, _: Context) -> Self::PingFut {
        future::ready(())
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let bind_addr = "/tmp/tarpc_on_unix_example.sock";

    let _ = std::fs::remove_file(bind_addr);

    let listener = UnixListener::bind(bind_addr).unwrap();
    let codec_builder = LengthDelimitedCodec::builder();
    tokio::spawn(async move {
        loop {
            let (conn, _addr) = listener.accept().await.unwrap();
            let framed = codec_builder.new_framed(conn);
            let transport = transport::new(framed, Bincode::default());

            let fut = BaseChannel::with_defaults(transport)
                .respond_with(Service.serve())
                .execute();
            tokio::spawn(fut);
        }
    });

    let conn = UnixStream::connect(bind_addr).await?;
    let transport = transport::new(codec_builder.new_framed(conn), Bincode::default());
    PingServiceClient::new(Default::default(), transport)
        .spawn()?
        .ping(tarpc::context::current())
        .await
}
