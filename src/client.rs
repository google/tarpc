// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use WireError;
use bincode::serde::DeserializeError;
use futures::{self, Future};
use protocol::Proto;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;
use tokio_core::io::Io;
use tokio_core::net::TcpStream;
use tokio_proto::BindClient as ProtoBindClient;
use tokio_proto::multiplex::Multiplex;
use tokio_service::Service;

#[cfg(feature = "tls")]
use self::tls::*;
#[cfg(feature = "tls")]
use tokio_tls::{TlsStream};


type WireResponse<Resp, E> = Result<Result<Resp, WireError<E>>, DeserializeError>;
type BindClient<Req, Resp, E> = <Proto<Req, Result<Resp, WireError<E>>> as ProtoBindClient<Multiplex, Either>>::BindClient;

#[cfg(feature = "tls")]
pub mod tls {
    use native_tls::TlsConnector;
    use super::*;

    /// TLS context
    pub struct TlsClientContext {
        /// Domain to connect to
        pub domain: String,
        /// TLS connector
        pub tls_connector: TlsConnector,
    }

    impl TlsClientContext {
        /// Try to make a new `TlsClientContext`, providing the domain the client will
        /// connect to.
        pub fn try_new<S: Into<String>>(domain: S)
                                        -> Result<TlsClientContext, ::native_tls::Error> {
            Ok(TlsClientContext {
                domain: domain.into(),
                tls_connector: TlsConnector::builder()?.build()?,
            })
        }
    }

    impl Config {
        /// Construct a new `Config`
        pub fn new_tls(tls_client_cx: TlsClientContext) -> Self {
            Config {
                tls_client_cx: Some(tls_client_cx),
            }
        }
    }
}

/// A client that impls `tokio_service::Service` that writes and reads bytes.
///
/// Typically, this would be combined with a serialization pre-processing step
/// and a deserialization post-processing step.
pub struct Client<Req, Resp, E>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static,
{
    inner: BindClient<Req, Resp, E>,
}

#[derive(Debug)]
pub enum Either {
    Tcp(TcpStream),
    #[cfg(feature = "tls")]
    Tls(TlsStream<TcpStream>),
}

impl From<TcpStream> for Either {
    fn from(stream: TcpStream) -> Self {
        Either::Tcp(stream)
    }
}

#[cfg(feature = "tls")]
impl From<TlsStream<TcpStream>> for Either {
    fn from(stream: TlsStream<TcpStream>) -> Self {
        Either::Tls(stream)
    }
}

impl io::Read for Either {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match *self {
            Either::Tcp(ref mut stream) => stream.read(buf),
            #[cfg(feature = "tls")]
            Either::Tls(ref mut stream) => stream.read(buf),
        }
    }
}

impl io::Write for Either {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match *self {
            Either::Tcp(ref mut stream) => stream.write(buf),
            #[cfg(feature = "tls")]
            Either::Tls(ref mut stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match *self {
            Either::Tcp(ref mut stream) => stream.flush(),
            #[cfg(feature = "tls")]
            Either::Tls(ref mut stream) => stream.flush(),
        }
    }
}

impl Io for Either { }

impl<Req, Resp, E> Clone for Client<Req, Resp, E>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static,
{
    fn clone(&self) -> Self {
        Client { inner: self.inner.clone() }
    }
}

impl<Req, Resp, E> Service for Client<Req, Resp, E>
    where Req: Serialize + Sync + Send + 'static,
          Resp: Deserialize + Sync + Send + 'static,
          E: Deserialize + Sync + Send + 'static,
{
    type Request = Req;
    type Response = Result<Resp, ::Error<E>>;
    type Error = io::Error;
    type Future = futures::Map<<BindClient<Req, Resp, E> as Service>::Future,
                 fn(WireResponse<Resp, E>) -> Result<Resp, ::Error<E>>>;

    fn call(&self, request: Self::Request) -> Self::Future {
        self.inner.call(request).map(Self::map_err)
    }
}

impl<Req, Resp, E> Client<Req, Resp, E>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static,
{
    fn new(inner: BindClient<Req, Resp, E>) -> Self
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static,
    {
        Client { inner: inner }
    }

    fn map_err(resp: WireResponse<Resp, E>) -> Result<Resp, ::Error<E>> {
        resp.map(|r| r.map_err(::Error::from))
            .map_err(::Error::ClientDeserialize)
            .and_then(|r| r)
    }
}

impl<Req, Resp, E> fmt::Debug for Client<Req, Resp, E>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "Client {{ .. }}")
    }
}

/// Client configuration when connecting.
#[derive(Default)]
pub struct Config {
    #[cfg(feature = "tls")]
    /// Tls configuration
    tls_client_cx: Option<TlsClientContext>,
}

#[cfg(feature = "tls")]
impl Config {
    /// Construct a new `Config`
    pub fn new_tcp() -> Self {
        Config {
            tls_client_cx: None,
        }
    }
}

#[cfg(not(feature = "tls"))]
impl Config {
    /// Construct a new `Config`
    pub fn new_tcp() -> Self {
        Config { }
    }
}

/// Exposes a trait for connecting asynchronously to servers.
pub mod future {
    use future::REMOTE;
    use futures::{self, Async, Future, future};
    use protocol::Proto;
    use serde::{Deserialize, Serialize};
    use std::io;
    use std::marker::PhantomData;
    use std::net::SocketAddr;
    use super::{Client, Config, Either};
    use tokio_core::net::TcpStream;
    use tokio_core::reactor;
    use tokio_proto::BindClient;
    use tokio_core::net::TcpStreamNew;
    #[cfg(feature = "tls")]
    use super::tls::TlsClientContext;
    #[cfg(feature = "tls")]
    use tokio_tls::{ConnectAsync, TlsStream, TlsConnectorExt};
    #[cfg(feature = "tls")]
    use errors::native2io;

    /// Types that can connect to a server asynchronously.
    pub trait Connect<'a>: Sized {
        /// The type of the future returned when calling `connect`.
        type ConnectFut: Future<Item = Self, Error = io::Error> + 'static;

        /// The type of the future returned when calling `connect_with`.
        type ConnectWithFut: Future<Item = Self, Error = io::Error> + 'a;
        /// Connects to a server located at the given address, using a remote to the default
        /// reactor.
        fn connect(addr: &SocketAddr, config: Config) -> Self::ConnectFut {
            Self::connect_remotely(addr, &REMOTE, config)
        }

        /// Connects to a server located at the given address, using the given reactor
        /// remote.
        fn connect_remotely(addr: &SocketAddr,
                            remote: &reactor::Remote,
                            config: Config)
                            -> Self::ConnectFut;

        /// Connects to a server located at the given address, using the given reactor
        /// handle.
        fn connect_with(addr: &SocketAddr,
                        handle: &'a reactor::Handle,
                        config: Config)
                        -> Self::ConnectWithFut;
    }

    /// A future that resolves to a `Client` or an `io::Error`.
    pub struct ConnectWithFuture<'a, Req, Resp, E, F>
        where F: Future
    {
        inner: futures::Map<F, MultiplexConnect<'a, Req, Resp, E>>,
    }

    /// Provides the connection Fn impl for Tls
    pub struct ConnectFn {
        #[cfg(feature = "tls")]
        tls_ctx: Option<TlsClientContext>
    }

    impl FnOnce<(TcpStream,)> for ConnectFn {
        #[cfg(feature = "tls")]
        type Output = future::Either<
            future::FutureResult<Either, io::Error>,
            futures::Map<
                futures::MapErr<
                    ConnectAsync<TcpStream>,
                    fn(::native_tls::Error) -> io::Error>,
                fn(TlsStream<TcpStream>) -> Either>>;
        #[cfg(not(feature = "tls"))]
        type Output = future::FutureResult<Either, io::Error>;

        extern "rust-call" fn call_once(self, (tcp,): (TcpStream,)) -> Self::Output {
            #[cfg(feature = "tls")]
            match self.tls_ctx {
                None => future::Either::A(future::ok(Either::from(tcp))),
                Some(tls_client_cx) => future::Either::B(tls_client_cx.tls_connector
                    .connect_async(&tls_client_cx.domain, tcp)
                    .map_err(native2io as fn(_) -> _)
                    .map(Either::from as fn(_) -> _)),
            }
            #[cfg(not(feature = "tls"))]
            future::ok(Either::from(tcp))
        }
    }

    type ConnectFut =
        futures::AndThen<TcpStreamNew,
                         <ConnectFn as FnOnce<(TcpStream,)>>::Output,
                         ConnectFn>;


    impl<'a, Req, Resp, E> Connect<'a> for Client<Req, Resp, E>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static
    {
        type ConnectFut = ConnectFuture<Req, Resp, E>;
        type ConnectWithFut = ConnectWithFuture<'a,
                          Req,
                          Resp,
                          E,
                          ConnectFut>;

        fn connect_remotely(addr: &SocketAddr,
                            remote: &reactor::Remote,
                            _config: Config) -> Self::ConnectFut {
            let addr = *addr;
            let (tx, rx) = futures::oneshot();
            remote.spawn(move |handle| {
                let handle2 = handle.clone();
                TcpStream::connect(&addr, handle)
                    .and_then(move |socket| {
                        #[cfg(feature = "tls")]
                        match _config.tls_client_cx {
                            Some(tls_client_cx) => {
                                future::Either::A(tls_client_cx.tls_connector
                                    .connect_async(&tls_client_cx.domain, socket)
                                    .map(Either::Tls)
                                    .map_err(native2io))
                            }
                            None => future::Either::B(future::ok(Either::Tcp(socket))),
                        }
                        #[cfg(not(feature = "tls"))]
                        future::ok(Either::Tcp(socket))
                    })
                    .map(move |tcp| Client::new(Proto::new().bind_client(&handle2, tcp)))
                    .then(move |result| {
                        tx.complete(result);
                        Ok(())
                    })
            });
            ConnectFuture { inner: rx }
        }

        fn connect_with(addr: &SocketAddr,
                        handle: &'a reactor::Handle,
                        _config: Config)
                        -> Self::ConnectWithFut {
            #[cfg(feature = "tls")]
            return ConnectWithFuture {
                inner: TcpStream::connect(addr, handle)
                    .and_then(ConnectFn {
                        tls_ctx: _config.tls_client_cx,
                    })
                    .map(MultiplexConnect::new(handle))
            };
            #[cfg(not(feature = "tls"))]
            return ConnectWithFuture {
                inner: TcpStream::connect(addr, handle)
                    .and_then(ConnectFn {})
                    .map(MultiplexConnect::new(handle))
            };
        }
    }

    /// A future that resolves to a `Client` or an `io::Error`.
    pub struct ConnectFuture<Req, Resp, E>
        where Req: Serialize + 'static,
              Resp: Deserialize + 'static,
              E: Deserialize + 'static,
    {
        inner: futures::Oneshot<io::Result<Client<Req, Resp, E>>>,
    }

    impl<Req, Resp, E> Future for ConnectFuture<Req, Resp, E>
        where Req: Serialize + 'static,
              Resp: Deserialize + 'static,
              E: Deserialize + 'static,
    {
        type Item = Client<Req, Resp, E>;
        type Error = io::Error;

        fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
            // Ok to unwrap because we ensure the oneshot is always completed.
            match self.inner.poll().unwrap() {
                Async::Ready(Ok(client)) => Ok(Async::Ready(client)),
                Async::Ready(Err(err)) => Err(err),
                Async::NotReady => Ok(Async::NotReady),
            }
        }
    }

    impl<'a, Req, Resp, E, F, I> Future for ConnectWithFuture<'a, Req, Resp, E, F>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static,
              F: Future<Item = I, Error = io::Error>,
              I: Into<Either>
    {
        type Item = Client<Req, Resp, E>;
        type Error = io::Error;

        fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
            self.inner.poll()
        }
    }

    struct MultiplexConnect<'a, Req, Resp, E>(&'a reactor::Handle, PhantomData<(Req, Resp, E)>);

    impl<'a, Req, Resp, E> MultiplexConnect<'a, Req, Resp, E> {
        fn new(handle: &'a reactor::Handle) -> Self {
            MultiplexConnect(handle, PhantomData)
        }
    }

    impl<'a, Req, Resp, E, I> FnOnce<(I,)> for MultiplexConnect<'a, Req, Resp, E>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static,
              I: Into<Either>,
    {
        type Output = Client<Req, Resp, E>;

        extern "rust-call" fn call_once(self, (stream,): (I,)) -> Self::Output {
            Client::new(Proto::new().bind_client(self.0, stream.into()))
        }
    }
}

/// Exposes a trait for connecting synchronously to servers.
pub mod sync {
    use futures::Future;
    use serde::{Deserialize, Serialize};
    use std::io;
    use std::net::ToSocketAddrs;
    use super::{Client, Config};

    /// Types that can connect to a server synchronously.
    pub trait Connect: Sized {
        /// Connects to a server located at the given address.
        fn connect<A>(addr: A, config: Config) -> Result<Self, io::Error> where A: ToSocketAddrs;
    }

    impl<Req, Resp, E> Connect for Client<Req, Resp, E>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static
    {
        fn connect<A>(addr: A, config: Config) -> Result<Self, io::Error>
            where A: ToSocketAddrs
        {
            let addr = if let Some(a) = addr.to_socket_addrs()?.next() {
                a
            } else {
                return Err(io::Error::new(io::ErrorKind::AddrNotAvailable,
                                          "`ToSocketAddrs::to_socket_addrs` returned an empty \
                                           iterator."));
            };
            <Self as super::future::Connect>::connect(&addr, config).wait()
        }
    }
}
