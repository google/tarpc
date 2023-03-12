use rustls_pemfile::{certs, pkcs8_private_keys};
use std::io::{BufReader, Cursor};
use std::net::{IpAddr, Ipv4Addr};

use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio_rustls::rustls::{self, OwnedTrustAnchor};
use tokio_rustls::{webpki, TlsAcceptor, TlsConnector};

use tarpc::context::Context;
use tarpc::serde_transport as transport;
use tarpc::server::{BaseChannel, Channel};
use tarpc::tokio_serde::formats::Bincode;
use tarpc::tokio_util::codec::length_delimited::LengthDelimitedCodec;

#[tarpc::service]
pub trait PingService {
    async fn ping() -> String;
}

#[derive(Clone)]
struct Service;

#[tarpc::server]
impl PingService for Service {
    async fn ping(self, _: Context) -> String {
        "🔒".to_owned()
    }
}

// certs were generated with mkcerts https://github.com/FiloSottile/mkcert
// 'mkcert localhost'
// ca public key is located in "$(mkcert -CAROOT)/rootCA.pem"
const CERT: &str = include_str!("certs/localhost.pem");
const CHAIN: &[u8] = include_bytes!("certs/rootCA.pem");
const RSA: &str = include_str!("certs/localhost-key.pem");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // -------------------- start here to setup tls tcp tokio stream --------------------------
    // ref certs and loading from: https://github.com/tokio-rs/tls/blob/master/tokio-rustls/tests/test.rs
    // ref basic tls server setup from: https://github.com/tokio-rs/tls/blob/master/tokio-rustls/examples/server/src/main.rs
    let cert = certs(&mut BufReader::new(Cursor::new(CERT)))
        .unwrap()
        .drain(..)
        .map(rustls::Certificate)
        .collect();
    let mut keys = pkcs8_private_keys(&mut BufReader::new(Cursor::new(RSA))).unwrap();
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

    // ref ./custom_transport.rs server side
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

    // ---------------------- client connection ---------------------
    // cert loading from: https://github.com/tokio-rs/tls/blob/357bc562483dcf04c1f8d08bd1a831b144bf7d4c/tokio-rustls/tests/test.rs#L113
    // tls client connection from https://github.com/tokio-rs/tls/blob/master/tokio-rustls/examples/client/src/main.rs
    let chain = certs(&mut std::io::Cursor::new(CHAIN)).unwrap();
    let mut root_store = rustls::RootCertStore::empty();
    root_store.add_server_trust_anchors(chain.iter().map(|cert| {
        let ta = webpki::TrustAnchor::try_from_cert_der(&cert[..]).unwrap();
        OwnedTrustAnchor::from_subject_spki_name_constraints(
            ta.subject,
            ta.spki,
            ta.name_constraints,
        )
    }));

    let config = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    let domain = rustls::ServerName::try_from("localhost")?;
    let connector = TlsConnector::from(Arc::new(config));

    let stream = TcpStream::connect(server_addr).await?;
    let stream = connector.connect(domain, stream).await?;

    let transport = transport::new(codec_builder.new_framed(stream), Bincode::default());
    PingServiceClient::new(Default::default(), transport)
        .spawn()
        .ping(tarpc::context::current())
        .await?;

    Ok(())
}
