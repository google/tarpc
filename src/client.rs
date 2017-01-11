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
use std::marker::PhantomData;
use tokio_core::io::Io;
use tokio_core::net::TcpStream;
use tokio_proto::BindClient as ProtoBindClient;
use tokio_proto::multiplex::Multiplex;
use tokio_service::Service;

type WireResponse<Resp, E> = Result<Result<Resp, WireError<E>>, DeserializeError>;
type BindClient<Req, Resp, E, S> = <Proto<Req, Result<Resp, WireError<E>>> as ProtoBindClient<Multiplex, S>>::BindClient;

#[cfg(feature = "tls")]
pub mod tls {
    use native_tls::TlsConnector;
    use super::*;
    use tokio_tls::TlsStream;

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

    impl Config<TlsStream<TcpStream>> {
        /// Construct a new `Config<TlsStream<TcpStream>>`
        pub fn new_tls(tls_client_cx: TlsClientContext) -> Self {
            Config {
                _stream: PhantomData,
                tls_client_cx: Some(tls_client_cx),
            }
        }
    }
}

#[cfg(feature = "tls")]
use self::tls::*;

/// A client that impls `tokio_service::Service` that writes and reads bytes.
///
/// Typically, this would be combined with a serialization pre-processing step
/// and a deserialization post-processing step.
pub struct Client<Req, Resp, E, S=TcpStream>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static,
          S: Io + 'static
{
    inner: BindClient<Req, Resp, E, S>,
}

impl<Req, Resp, E, S> Clone for Client<Req, Resp, E, S>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static,
          S: Io + 'static
{
    fn clone(&self) -> Self {
        Client { inner: self.inner.clone() }
    }
}

impl<Req, Resp, E, I> Service for Client<Req, Resp, E, I>
    where Req: Serialize + Sync + Send + 'static,
          Resp: Deserialize + Sync + Send + 'static,
          E: Deserialize + Sync + Send + 'static,
          I: Io + 'static,
{
    type Request = Req;
    type Response = Result<Resp, ::Error<E>>;
    type Error = io::Error;
    type Future = futures::Map<<BindClient<Req, Resp, E, I> as Service>::Future,
                 fn(WireResponse<Resp, E>) -> Result<Resp, ::Error<E>>>;

    fn call(&self, request: Self::Request) -> Self::Future {
        self.inner.call(request).map(Self::map_err)
    }
}

impl<Req, Resp, E, S> Client<Req, Resp, E, S>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static,
          S: Io + 'static
{
    fn new(inner: BindClient<Req, Resp, E, S>) -> Self
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static,
              S: Io + 'static
    {
        Client { inner: inner }
    }

    fn map_err(resp: WireResponse<Resp, E>) -> Result<Resp, ::Error<E>> {
        resp.map(|r| r.map_err(::Error::from))
            .map_err(::Error::ClientDeserialize)
            .and_then(|r| r)
    }
}

impl<Req, Resp, E, S> fmt::Debug for Client<Req, Resp, E, S>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static,
          S: Io + 'static
{
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "Client {{ .. }}")
    }
}

/// TODO:
pub struct Config<S> {
    _stream: PhantomData<S>,
    #[cfg(feature = "tls")]
    tls_client_cx: Option<TlsClientContext>,
}

#[cfg(feature = "tls")]
impl<S> Default for Config<S> {
    fn default() -> Self {
        Config {
            _stream: PhantomData,
            tls_client_cx: None,
        }
    }
}

#[cfg(not(feature = "tls"))]
impl<S> Default for Config<S> {
    fn default() -> Self {
        Config {
            _stream: PhantomData,
        }
    }
}

#[cfg(feature = "tls")]
impl Config<TcpStream> {
    /// Construct a new `Config<TcpStream>`
    pub fn new_tcp() -> Self {
        Config {
            _stream: PhantomData,
            tls_client_cx: None,
        }
    }
}

#[cfg(not(feature = "tls"))]
impl Config<TcpStream> {
    /// Construct a new `Config<TcpStream>`
    pub fn new_tcp() -> Self {
        Config { _stream: PhantomData }
    }
}

/// Exposes a trait for connecting asynchronously to servers.
pub mod future {
    use future::REMOTE;
    use futures::{self, Async, Future};
    use protocol::Proto;
    use serde::{Deserialize, Serialize};
    use std::io;
    use std::marker::PhantomData;
    use std::net::SocketAddr;
    use super::{Client, Config};
    use tokio_core::io::Io;
    use tokio_core::net::TcpStream;
    use tokio_core::reactor;
    use tokio_proto::BindClient;

    /// Types that can connect to a server asynchronously.
    pub trait Connect<'a, S>: Sized
        where S: Io
    {
        /// The type of the future returned when calling `connect`.
        type ConnectFut: Future<Item = Self, Error = io::Error> + 'static;

        /// The type of the future returned when calling `connect_with`.
        type ConnectWithFut: Future<Item = Self, Error = io::Error> + 'a;
        /// Connects to a server located at the given address, using a remote to the default
        /// reactor.
        fn connect(addr: &SocketAddr, config: Config<S>) -> Self::ConnectFut {
            Self::connect_remotely(addr, &REMOTE, config)
        }

        /// Connects to a server located at the given address, using the given reactor
        /// remote.
        fn connect_remotely(addr: &SocketAddr,
                            remote: &reactor::Remote,
                            config: Config<S>)
                            -> Self::ConnectFut;

        /// Connects to a server located at the given address, using the given reactor
        /// handle.
        fn connect_with(addr: &SocketAddr,
                        handle: &'a reactor::Handle,
                        config: Config<S>)
                        -> Self::ConnectWithFut;
    }

    /// A future that resolves to a `Client` or an `io::Error`.
    pub struct ConnectWithFuture<'a, Req, Resp, E, S, F>
        where S: Io,
              F: Future<Item = S, Error = io::Error>
    {
        inner: futures::Map<F, MultiplexConnect<'a, Req, Resp, E>>,
    }

    #[cfg(feature = "tls")]
    mod tls {
        use errors::native2io;
        use super::*;
        use super::super::tls::TlsClientContext;
        use tokio_core::net::TcpStreamNew;
        use tokio_tls::{ConnectAsync, TlsStream, TlsConnectorExt};

        /// Provides the connection Fn impl for Tls
        pub struct TlsConnectFn;

        impl FnOnce<(((TcpStream, TlsClientContext),))> for TlsConnectFn {
            type Output = futures::MapErr<ConnectAsync<TcpStream>,
                            fn(::native_tls::Error) -> io::Error>;

            extern "rust-call" fn call_once(self,
                                            ((tcp, tls_client_cx),): ((TcpStream,
                                                                       TlsClientContext),))
                                            -> Self::Output {
                tls_client_cx.tls_connector
                    .connect_async(&tls_client_cx.domain, tcp)
                    .map_err(native2io)
            }
        }

        type TlsConnectFut =
            futures::AndThen<futures::Join<TcpStreamNew,
                                           futures::future::FutureResult<TlsClientContext,
                                                                         io::Error>>,
                             futures::MapErr<ConnectAsync<TcpStream>,
                                             fn(::native_tls::Error) -> io::Error>,
                             TlsConnectFn>;

        impl<'a, Req, Resp, E> Connect<'a, TlsStream<TcpStream>>
            for Client<Req, Resp, E, TlsStream<TcpStream>>
            where Req: Serialize + Sync + Send + 'static,
                  Resp: Deserialize + Sync + Send + 'static,
                  E: Deserialize + Sync + Send + 'static
        {
            type ConnectFut = ConnectFuture<Req, Resp, E, TlsStream<TcpStream>>;
            type ConnectWithFut = ConnectWithFuture<'a,
                              Req,
                              Resp,
                              E,
                              TlsStream<TcpStream>,
                              TlsConnectFut>;

            fn connect_remotely(addr: &SocketAddr,
                                remote: &reactor::Remote,
                                config: Config<TlsStream<TcpStream>>)
                                -> Self::ConnectFut {
                let addr = *addr;
                let (tx, rx) = futures::oneshot();
                remote.spawn(move |handle| {
                    let handle2 = handle.clone();
                    TcpStream::connect(&addr, handle)
                        .and_then(move |socket| {
                            let tls_client_cx = config.tls_client_cx
                                .expect("Need TlsClientContext for a TlsStream");
                            tls_client_cx.tls_connector
                                .connect_async(&tls_client_cx.domain, socket)
                                .map_err(native2io)
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
                            config: Config<TlsStream<TcpStream>>)
                            -> Self::ConnectWithFut {
                let tls_client_cx = config.tls_client_cx
                    .expect("Need TlsClientContext for a TlsStream");
                ConnectWithFuture {
                    inner: TcpStream::connect(addr, handle)
                        .join(futures::finished(tls_client_cx))
                        .and_then(TlsConnectFn)
                        .map(MultiplexConnect::new(handle)),
                }
            }
        }
    }

    /// A future that resolves to a `Client` or an `io::Error`.
    pub struct ConnectFuture<Req, Resp, E, S>
        where Req: Serialize + 'static,
              Resp: Deserialize + 'static,
              E: Deserialize + 'static,
              S: Io + 'static
    {
        inner: futures::Oneshot<io::Result<Client<Req, Resp, E, S>>>,
    }

    impl<Req, Resp, E, S> Future for ConnectFuture<Req, Resp, E, S>
        where Req: Serialize + 'static,
              Resp: Deserialize + 'static,
              E: Deserialize + 'static,
              S: Io + 'static
    {
        type Item = Client<Req, Resp, E, S>;
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

    impl<'a, Req, Resp, E, S, F> Future for ConnectWithFuture<'a, Req, Resp, E, S, F>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static,
              S: Io + 'static,
              F: Future<Item = S, Error = io::Error>
    {
        type Item = Client<Req, Resp, E, S>;
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

    impl<'a, Req, Resp, E, S> FnOnce<(S,)> for MultiplexConnect<'a, Req, Resp, E>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static,
              S: Io + 'static
    {
        type Output = Client<Req, Resp, E, S>;

        extern "rust-call" fn call_once(self, (s,): (S,)) -> Self::Output {
            Client::new(Proto::new().bind_client(self.0, s))
        }
    }

    impl<'a, Req, Resp, E> Connect<'a, TcpStream> for Client<Req, Resp, E, TcpStream>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static
    {
        type ConnectFut = ConnectFuture<Req, Resp, E, TcpStream>;
        type ConnectWithFut = ConnectWithFuture<'a,
                          Req,
                          Resp,
                          E,
                          TcpStream,
                          ::tokio_core::net::TcpStreamNew>;

        fn connect_remotely(addr: &SocketAddr,
                            remote: &reactor::Remote,
                            _config: Config<TcpStream>)
                            -> Self::ConnectFut {
            let addr = *addr;
            let (tx, rx) = futures::oneshot();
            remote.spawn(move |handle| {
                let handle2 = handle.clone();
                TcpStream::connect(&addr, handle)
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
                        _config: Config<TcpStream>)
                        -> Self::ConnectWithFut {
            ConnectWithFuture {
                inner: TcpStream::connect(addr, handle).map(MultiplexConnect::new(handle)),
            }
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
    use tokio_core::io::Io;
    use tokio_core::net::TcpStream;

    /// Types that can connect to a server synchronously.
    pub trait Connect<S>: Sized
        where S: Io
    {
        /// Connects to a server located at the given address.
        fn connect<A>(addr: A, config: Config<S>) -> Result<Self, io::Error> where A: ToSocketAddrs;
    }

    impl<Req, Resp, E> Connect<TcpStream> for Client<Req, Resp, E, TcpStream>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static
    {
        fn connect<A>(addr: A, config: Config<TcpStream>) -> Result<Self, io::Error>
            where A: ToSocketAddrs
        {
            let addr = if let Some(a) = addr.to_socket_addrs()?.next() {
                a
            } else {
                return Err(io::Error::new(io::ErrorKind::AddrNotAvailable,
                                          "`ToSocketAddrs::to_socket_addrs` returned an empty \
                                           iterator."));
            };
            <Self as super::future::Connect<TcpStream>>::connect(&addr, config).wait()
        }
    }

    cfg_if! {
        if #[cfg(feature = "tls")] {
            use ::tokio_tls::TlsStream;

            impl<Req, Resp, E> Connect<TlsStream<TcpStream>> for Client<Req, Resp, E, TlsStream<TcpStream>>
                where Req: Serialize + Sync + Send + 'static,
                      Resp: Deserialize + Sync + Send + 'static,
                      E: Deserialize + Sync + Send + 'static
            {
                fn connect<A>(addr: A, config: Config<TlsStream<TcpStream>>) -> Result<Self, io::Error>
                    where A: ToSocketAddrs
                {
                    let addr = if let Some(a) = addr.to_socket_addrs()?.next() {
                        a
                    } else {
                        return Err(io::Error::new(io::ErrorKind::AddrNotAvailable,
                                                  "`ToSocketAddrs::to_socket_addrs` returned an \
                                                   empty iterator."));
                    };
                    <Self as super::future::Connect<TlsStream<TcpStream>>>::connect(&addr, config).wait()
                }
            }
        } else {}
    }
}
