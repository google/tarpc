use rustls_pemfile::{certs, rsa_private_keys};
use std::io::{BufReader, Cursor};
use std::net::{IpAddr, Ipv4Addr};

use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_rustls::rustls::{self};
use tokio_rustls::TlsAcceptor;

use tarpc::context::Context;
use tarpc::{serde_transport as transport};
use tarpc::server::{BaseChannel, Channel};
use tarpc::tokio_serde::formats::Bincode;
use tarpc::tokio_util::codec::length_delimited::LengthDelimitedCodec;

#[tarpc::service]
pub trait PingService {
    async fn ping();
}

#[derive(Clone)]
struct Service;

#[tarpc::server]
impl PingService for Service {
    async fn ping(self, _: Context) {}
}


// ref certs and loading from: https://github.com/tokio-rs/tls/blob/master/tokio-rustls/tests/test.rs
// ref basic tls server setup from: https://github.com/tokio-rs/tls/blob/master/tokio-rustls/examples/server/src/main.rs
const CERT: &str = include_str!("certs/end.cert");
// const CHAIN: &[u8] = include_bytes!("certs/end.chain");
const RSA: &str = include_str!("certs/end.rsa");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
   // -------------------- start here to setup tls tcp tokio stream --------------------------
    // https://github.com/tokio-rs/tls/blob/master/tokio-rustls/examples/server/src/main.rs
    // https://github.com/tokio-rs/tls/blob/master/tokio-rustls/tests/test.rs

    let cert = certs(&mut BufReader::new(Cursor::new(CERT)))
        .unwrap()
        .drain(..)
        .map(rustls::Certificate)
        .collect();
    let mut keys = rsa_private_keys(&mut BufReader::new(Cursor::new(RSA))).unwrap();
    let mut keys = keys.drain(..).map(rustls::PrivateKey);

    let server_addr = (IpAddr::V4(Ipv4Addr::LOCALHOST), 5000);

    let config = rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(cert, keys.next().unwrap())
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind(&server_addr).await.unwrap();
    let codec_builder = LengthDelimitedCodec::builder();

    tokio::spawn(async move {
        loop {
            let (stream, _peer_addr) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            let tls_stream = acceptor.accept(stream).await.unwrap();
            let framed = codec_builder.new_framed(tls_stream);

            let transport = transport::new(framed, Bincode::default());

            let fut = BaseChannel::with_defaults(transport).execute(Service.serve());
            tokio::spawn(fut);
        }
    });

    Ok(())
}
