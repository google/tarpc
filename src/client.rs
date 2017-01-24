// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use {Reactor, WireError};
use bincode::serde::DeserializeError;
use futures::{self, Future};
use protocol::Proto;
#[cfg(feature = "tls")]
use self::tls::*;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;
use stream_type::StreamType;
use tokio_core::reactor;
use tokio_proto::BindClient as ProtoBindClient;
use tokio_proto::multiplex::Multiplex;
use tokio_service::Service;

type WireResponse<Resp, E> = Result<Result<Resp, WireError<E>>, DeserializeError>;
type ResponseFuture<Req, Resp, E> = futures::Map<<BindClient<Req, Resp, E> as Service>::Future,
                                            fn(WireResponse<Resp, E>) -> Result<Resp, ::Error<E>>>;
type BindClient<Req, Resp, E> = <Proto<Req, Result<Resp, WireError<E>>> as
                                ProtoBindClient<Multiplex, StreamType>>::BindClient;

/// TLS-specific functionality
#[cfg(feature = "tls")]
pub mod tls {
    use native_tls::TlsConnector;

    /// TLS context
    pub struct TlsClientContext {
        /// Domain to connect to
        pub domain: String,
        /// TLS connector
        pub tls_connector: TlsConnector,
    }

    impl TlsClientContext {
        /// Try to construct a new `TlsClientContext`, providing the domain the client will
        /// connect to.
        pub fn new<S: Into<String>>(domain: S) -> Result<Self, ::native_tls::Error> {
            Ok(TlsClientContext {
                domain: domain.into(),
                tls_connector: TlsConnector::builder()?.build()?,
            })
        }

        /// Construct a new `TlsClientContext` using the provided domain and `TlsConnector`
        pub fn from_connector<S: Into<String>>(domain: S, tls_connector: TlsConnector) -> Self {
            TlsClientContext {
                domain: domain.into(),
                tls_connector: tls_connector,
            }
        }
    }
}

/// A client that impls `tokio_service::Service` that writes and reads bytes.
///
/// Typically, this would be combined with a serialization pre-processing step
/// and a deserialization post-processing step.
#[doc(hidden)]
pub struct Client<Req, Resp, E>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static
{
    inner: BindClient<Req, Resp, E>,
}

impl<Req, Resp, E> Clone for Client<Req, Resp, E>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static
{
    fn clone(&self) -> Self {
        Client { inner: self.inner.clone() }
    }
}

impl<Req, Resp, E> Service for Client<Req, Resp, E>
    where Req: Serialize + Sync + Send + 'static,
          Resp: Deserialize + Sync + Send + 'static,
          E: Deserialize + Sync + Send + 'static
{
    type Request = Req;
    type Response = Result<Resp, ::Error<E>>;
    type Error = io::Error;
    type Future = ResponseFuture<Req, Resp, E>;

    fn call(&self, request: Self::Request) -> Self::Future {
        self.inner.call(request).map(Self::map_err)
    }
}

impl<Req, Resp, E> Client<Req, Resp, E>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static
{
    fn new(inner: BindClient<Req, Resp, E>) -> Self
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static
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
          E: Deserialize + 'static
{
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "Client {{ .. }}")
    }
}

/// Additional options to configure how the client connects and operates.
#[derive(Default)]
pub struct Options {
    reactor: Option<Reactor>,
    #[cfg(feature = "tls")]
    tls_ctx: Option<TlsClientContext>,
}

impl Options {
    /// Connect using the given reactor handle.
    pub fn handle(mut self, handle: reactor::Handle) -> Self {
        self.reactor = Some(Reactor::Handle(handle));
        self
    }

    /// Connect using the given reactor remote.
    pub fn remote(mut self, remote: reactor::Remote) -> Self {
        self.reactor = Some(Reactor::Remote(remote));
        self
    }

    /// Connect using the given `TlsClientContext`
    #[cfg(feature = "tls")]
    pub fn tls(mut self, tls_ctx: TlsClientContext) -> Self {
        self.tls_ctx = Some(tls_ctx);
        self
    }
}

/// Exposes a trait for connecting asynchronously to servers.
pub mod future {
    use {REMOTE, Reactor};
    use futures::{self, Async, Future, future};
    use protocol::Proto;
    use serde::{Deserialize, Serialize};
    use std::io;
    use std::marker::PhantomData;
    use std::net::SocketAddr;
    use stream_type::StreamType;
    use super::{Client, Options};
    use tokio_core::net::{TcpStream, TcpStreamNew};
    use tokio_core::reactor;
    use tokio_proto::BindClient;
    cfg_if! {
        if  #[cfg(feature = "tls")] {
            use tokio_tls::{ConnectAsync, TlsStream, TlsConnectorExt};
            use super::tls::TlsClientContext;
            use errors::native_to_io;
        } else {}
    }

    /// Types that can connect to a server asynchronously.
    pub trait Connect: Sized {
        /// The type of the future returned when calling `connect`.
        type ConnectFut: Future<Item = Self, Error = io::Error>;

        /// Connects to a server located at the given address, using the given options.
        fn connect(addr: SocketAddr, options: Options) -> Self::ConnectFut;
    }

    type ConnectFutureInner<Req, Resp, E, T> = future::Either<futures::Map<futures::AndThen<
        TcpStreamNew, T, ConnectFn>, MultiplexConnect<Req, Resp, E>>, futures::Flatten<
        futures::MapErr<futures::Oneshot<io::Result<Client<Req, Resp, E>>>,
        fn(futures::Canceled) -> io::Error>>>;

    /// A future that resolves to a `Client` or an `io::Error`.
    #[doc(hidden)]
    pub struct ConnectFuture<Req, Resp, E>
        where Req: Serialize + 'static,
              Resp: Deserialize + 'static,
              E: Deserialize + 'static
    {
        #[cfg(not(feature = "tls"))]
        #[allow(unknown_lints, type_complexity)]
        inner: ConnectFutureInner<Req, Resp, E, future::FutureResult<StreamType, io::Error>>,
        #[cfg(feature = "tls")]
        #[allow(unknown_lints, type_complexity)]
        inner: ConnectFutureInner<Req, Resp, E, future::Either<future::FutureResult<
            StreamType, io::Error>, futures::Map<futures::MapErr<ConnectAsync<TcpStream>,
            fn(::native_tls::Error) -> io::Error>, fn(TlsStream<TcpStream>) -> StreamType>>>,
    }

    impl<Req, Resp, E> Future for ConnectFuture<Req, Resp, E>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static
    {
        type Item = Client<Req, Resp, E>;
        type Error = io::Error;

        fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
            // Ok to unwrap because we ensure the oneshot is always completed.
            match Future::poll(&mut self.inner)? {
                Async::Ready(client) => Ok(Async::Ready(client)),
                Async::NotReady => Ok(Async::NotReady),
            }
        }
    }

    struct MultiplexConnect<Req, Resp, E>(reactor::Handle, PhantomData<(Req, Resp, E)>);

    impl<Req, Resp, E> MultiplexConnect<Req, Resp, E> {
        fn new(handle: reactor::Handle) -> Self {
            MultiplexConnect(handle, PhantomData)
        }
    }

    impl<Req, Resp, E, I> FnOnce<(I,)> for MultiplexConnect<Req, Resp, E>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static,
              I: Into<StreamType>
    {
        type Output = Client<Req, Resp, E>;

        extern "rust-call" fn call_once(self, (stream,): (I,)) -> Self::Output {
            Client::new(Proto::new().bind_client(&self.0, stream.into()))
        }
    }

    /// Provides the connection Fn impl for Tls
    struct ConnectFn {
        #[cfg(feature = "tls")]
        tls_ctx: Option<TlsClientContext>,
    }

    impl FnOnce<(TcpStream,)> for ConnectFn {
        #[cfg(feature = "tls")]
        type Output = future::Either<future::FutureResult<StreamType, io::Error>,
                       futures::Map<futures::MapErr<ConnectAsync<TcpStream>,
                                                    fn(::native_tls::Error)
                                                       -> io::Error>,
                                    fn(TlsStream<TcpStream>) -> StreamType>>;
        #[cfg(not(feature = "tls"))]
        type Output = future::FutureResult<StreamType, io::Error>;

        extern "rust-call" fn call_once(self, (tcp,): (TcpStream,)) -> Self::Output {
            #[cfg(feature = "tls")]
            match self.tls_ctx {
                None => future::Either::A(future::ok(StreamType::from(tcp))),
                Some(tls_ctx) => {
                    future::Either::B(tls_ctx.tls_connector
                        .connect_async(&tls_ctx.domain, tcp)
                        .map_err(native_to_io as fn(_) -> _)
                        .map(StreamType::from as fn(_) -> _))
                }
            }
            #[cfg(not(feature = "tls"))]
            future::ok(StreamType::from(tcp))
        }
    }

    impl<Req, Resp, E> Connect for Client<Req, Resp, E>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static
    {
        type ConnectFut = ConnectFuture<Req, Resp, E>;

        fn connect(addr: SocketAddr, options: Options) -> Self::ConnectFut {
            // we need to do this for tls because we need to avoid moving the entire `Options`
            // struct into the `setup` closure, since `Reactor` is not `Send`.
            #[cfg(feature = "tls")]
            let mut options = options;
            #[cfg(feature = "tls")]
            let tls_ctx = options.tls_ctx.take();

            let setup = move |tx: futures::sync::oneshot::Sender<_>| {
                move |handle: &reactor::Handle| {
                    let handle2 = handle.clone();
                    TcpStream::connect(&addr, handle)
                        .and_then(move |socket| {
                            #[cfg(feature = "tls")]
                            match tls_ctx {
                                Some(tls_ctx) => {
                                    future::Either::A(tls_ctx.tls_connector
                                        .connect_async(&tls_ctx.domain, socket)
                                        .map(StreamType::Tls)
                                        .map_err(native_to_io))
                                }
                                None => future::Either::B(future::ok(StreamType::Tcp(socket))),
                            }
                            #[cfg(not(feature = "tls"))]
                            future::ok(StreamType::Tcp(socket))
                        })
                        .map(move |tcp| Client::new(Proto::new().bind_client(&handle2, tcp)))
                        .then(move |result| {
                            tx.complete(result);
                            Ok(())
                        })
                }
            };

            let rx = match options.reactor {
                Some(Reactor::Handle(handle)) => {
                    #[cfg(feature = "tls")]
                    let connect_fn = ConnectFn { tls_ctx: options.tls_ctx };
                    #[cfg(not(feature = "tls"))]
                    let connect_fn = ConnectFn {};
                    let tcp = TcpStream::connect(&addr, &handle)
                        .and_then(connect_fn)
                        .map(MultiplexConnect::new(handle));
                    return ConnectFuture { inner: future::Either::A(tcp) };
                }
                Some(Reactor::Remote(remote)) => {
                    let (tx, rx) = futures::oneshot();
                    remote.spawn(setup(tx));
                    rx
                }
                None => {
                    let (tx, rx) = futures::oneshot();
                    REMOTE.spawn(setup(tx));
                    rx
                }
            };
            fn panic(canceled: futures::Canceled) -> io::Error {
                unreachable!(canceled)
            }
            ConnectFuture { inner: future::Either::B(rx.map_err(panic as fn(_) -> _).flatten()) }
        }
    }
}

/// Exposes a trait for connecting synchronously to servers.
pub mod sync {
    use client::future::Connect as FutureConnect;
    use futures::{Future, future};
    use serde::{Deserialize, Serialize};
    use std::io;
    use std::net::ToSocketAddrs;
    use super::{Client, Options};
    use util::FirstSocketAddr;

    /// Types that can connect to a server synchronously.
    pub trait Connect: Sized {
        /// Connects to a server located at the given address.
        fn connect<A>(addr: A, options: Options) -> Result<Self, io::Error> where A: ToSocketAddrs;
    }

    impl<Req, Resp, E> Connect for Client<Req, Resp, E>
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static
    {
        fn connect<A>(addr: A, options: Options) -> Result<Self, io::Error>
            where A: ToSocketAddrs
        {
            let addr = addr.try_first_socket_addr()?;

            // Wrapped in a lazy future to ensure execution occurs when a task is present.
            future::lazy(move || <Self as FutureConnect>::connect(addr, options)).wait()
        }
    }
}
