// Copyright 2023 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use futures::prelude::*;
use rustls_pemfile::certs;
use std::io::{BufReader, Cursor};
use std::net::{IpAddr, Ipv4Addr};
use tokio_rustls::rustls::server::AllowAnyAuthenticatedClient;

use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio_rustls::rustls::{self, RootCertStore};
use tokio_rustls::{TlsAcceptor, TlsConnector};

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

impl PingService for Service {
    async fn ping(self, _: Context) -> String {
        "🔒".to_owned()
    }
}

// certs were generated with openssl 3 https://github.com/rustls/rustls/tree/main/test-ca
// used on client-side for server tls
const END_CHAIN: &str = include_str!("certs/eddsa/end.chain");
// used on client-side for client-auth
const CLIENT_PRIVATEKEY_CLIENT_AUTH: &str = include_str!("certs/eddsa/client.key");
const CLIENT_CERT_CLIENT_AUTH: &str = include_str!("certs/eddsa/client.cert");

// used on server-side for server tls
const END_CERT: &str = include_str!("certs/eddsa/end.cert");
const END_PRIVATEKEY: &str = include_str!("certs/eddsa/end.key");
// used on server-side for client-auth
const CLIENT_CHAIN_CLIENT_AUTH: &str = include_str!("certs/eddsa/client.chain");

pub fn load_certs(data: &str) -> Vec<rustls::Certificate> {
    certs(&mut BufReader::new(Cursor::new(data)))
        .unwrap()
        .into_iter()
        .map(rustls::Certificate)
        .collect()
}

pub fn load_private_key(key: &str) -> rustls::PrivateKey {
    let mut reader = BufReader::new(Cursor::new(key));
    loop {
        match rustls_pemfile::read_one(&mut reader).expect("cannot parse private key .pem file") {
            Some(rustls_pemfile::Item::RSAKey(key)) => return rustls::PrivateKey(key),
            Some(rustls_pemfile::Item::PKCS8Key(key)) => return rustls::PrivateKey(key),
            Some(rustls_pemfile::Item::ECKey(key)) => return rustls::PrivateKey(key),
            None => break,
            _ => {}
        }
    }
    panic!("no keys found in {:?} (encrypted keys not supported)", key);
}

async fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
    tokio::spawn(fut);
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // -------------------- start here to setup tls tcp tokio stream --------------------------
    // ref certs and loading from: https://github.com/tokio-rs/tls/blob/master/tokio-rustls/tests/test.rs
    // ref basic tls server setup from: https://github.com/tokio-rs/tls/blob/master/tokio-rustls/examples/server/src/main.rs
    let cert = load_certs(END_CERT);
    let key = load_private_key(END_PRIVATEKEY);
    let server_addr = (IpAddr::V4(Ipv4Addr::LOCALHOST), 5000);

    // ------------- server side client_auth cert loading start
    let mut client_auth_roots = RootCertStore::empty();
    for root in load_certs(CLIENT_CHAIN_CLIENT_AUTH) {
        client_auth_roots.add(&root).unwrap();
    }
    let client_auth = AllowAnyAuthenticatedClient::new(client_auth_roots);
    // ------------- server side client_auth cert loading end

    let config = rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_client_cert_verifier(client_auth) // use .with_no_client_auth() instead if you don't want client-auth
        .with_single_cert(cert, key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind(&server_addr).await.unwrap();
    let codec_builder = LengthDelimitedCodec::builder();

    // ref ./custom_transport.rs server side
    tokio::spawn(async move {
        loop {
            let (stream, _peer_addr) = listener.accept().await.unwrap();
            let tls_stream = acceptor.accept(stream).await.unwrap();
            let framed = codec_builder.new_framed(tls_stream);

            let transport = transport::new(framed, Bincode::default());

            let fut = BaseChannel::with_defaults(transport)
                .execute(Service.serve())
                .for_each(spawn);
            tokio::spawn(fut);
        }
    });

    // ---------------------- client connection ---------------------
    // tls client connection from https://github.com/tokio-rs/tls/blob/master/tokio-rustls/examples/client/src/main.rs
    let mut root_store = rustls::RootCertStore::empty();
    for root in load_certs(END_CHAIN) {
        root_store.add(&root).unwrap();
    }

    let client_auth_private_key = load_private_key(CLIENT_PRIVATEKEY_CLIENT_AUTH);
    let client_auth_certs = load_certs(CLIENT_CERT_CLIENT_AUTH);

    let config = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_store)
        .with_single_cert(client_auth_certs, client_auth_private_key)?; // use .with_no_client_auth() instead if you don't want client-auth

    let domain = rustls::ServerName::try_from("localhost")?;
    let connector = TlsConnector::from(Arc::new(config));

    let stream = TcpStream::connect(server_addr).await?;
    let stream = connector.connect(domain, stream).await?;

    let transport = transport::new(codec_builder.new_framed(stream), Bincode::default());
    let answer = PingServiceClient::new(Default::default(), transport)
        .spawn()
        .ping(tarpc::context::current())
        .await?;

    println!("ping answer: {answer}");

    Ok(())
}
