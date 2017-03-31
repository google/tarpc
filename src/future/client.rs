// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use {REMOTE, bincode};
use future::server::Response;
use futures::{self, Future, future};
use protocol::Proto;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;
use std::net::SocketAddr;
use stream_type::StreamType;
use tokio_core::net::TcpStream;
use tokio_core::reactor;
use tokio_proto::BindClient as ProtoBindClient;
use tokio_proto::multiplex::ClientService;
use tokio_service::Service;

cfg_if! {
    if #[cfg(feature = "tls")] {
        use errors::native_to_io;
        use tls::client::Context;
        use tokio_tls::TlsConnectorExt;
    } else {}
}

/// Additional options to configure how the client connects and operates.
#[derive(Debug)]
pub struct Options {
    /// Max packet size in bytes.
    max_payload_size: u64,
    reactor: Option<Reactor>,
    #[cfg(feature = "tls")]
    tls_ctx: Option<Context>,
}

impl Default for Options {
    #[cfg(feature = "tls")]
    fn default() -> Self {
        Options {
            max_payload_size: 2 << 20,
            reactor: None,
            tls_ctx: None,
        }
    }

    #[cfg(not(feature = "tls"))]
    fn default() -> Self {
        Options {
            max_payload_size: 2 << 20,
            reactor: None,
        }
    }
}

impl Options {
    /// Set the max payload size in bytes. The default is 2 << 20 (2 MiB).
    pub fn max_payload_size(mut self, bytes: u64) -> Self {
        self.max_payload_size = bytes;
        self
    }

    /// Drive using the given reactor handle.
    pub fn handle(mut self, handle: reactor::Handle) -> Self {
        self.reactor = Some(Reactor::Handle(handle));
        self
    }

    /// Drive using the given reactor remote.
    pub fn remote(mut self, remote: reactor::Remote) -> Self {
        self.reactor = Some(Reactor::Remote(remote));
        self
    }

    /// Connect using the given `Context`
    #[cfg(feature = "tls")]
    pub fn tls(mut self, tls_ctx: Context) -> Self {
        self.tls_ctx = Some(tls_ctx);
        self
    }
}

enum Reactor {
    Handle(reactor::Handle),
    Remote(reactor::Remote),
}

impl fmt::Debug for Reactor {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        const HANDLE: &'static &'static str = &"Reactor::Handle";
        const HANDLE_INNER: &'static &'static str = &"Handle { .. }";
        const REMOTE: &'static &'static str = &"Reactor::Remote";
        const REMOTE_INNER: &'static &'static str = &"Remote { .. }";

        match *self {
            Reactor::Handle(_) => f.debug_tuple(HANDLE).field(HANDLE_INNER).finish(),
            Reactor::Remote(_) => f.debug_tuple(REMOTE).field(REMOTE_INNER).finish(),
        }
    }
}
#[doc(hidden)]
pub struct Client<Req, Resp, E>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static
{
    inner: ClientService<StreamType, Proto<Req, Response<Resp, E>>>,
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
    type Response = Resp;
    type Error = ::Error<E>;
    type Future = ResponseFuture<Req, Resp, E>;

    fn call(&self, request: Self::Request) -> Self::Future {
        fn identity<T>(t: T) -> T {
            t
        }
        self.inner
            .call(request)
            .map(Self::map_err as _)
            .map_err(::Error::from as _)
            .and_then(identity as _)
    }
}

impl<Req, Resp, E> Client<Req, Resp, E>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static
{
    fn bind(handle: &reactor::Handle, tcp: StreamType, max_payload_size: u64) -> Self
        where Req: Serialize + Sync + Send + 'static,
              Resp: Deserialize + Sync + Send + 'static,
              E: Deserialize + Sync + Send + 'static
    {
        let inner = Proto::new(max_payload_size).bind_client(&handle, tcp);
        Client { inner }
    }

    fn map_err(resp: WireResponse<Resp, E>) -> Result<Resp, ::Error<E>> {
        resp.map(|r| r.map_err(::Error::from))
            .map_err(::Error::ResponseDeserialize)
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

/// Extension methods for clients.
pub trait ClientExt: Sized {
    /// The type of the future returned when calling `connect`.
    type ConnectFut: Future<Item = Self, Error = io::Error>;

    /// Connects to a server located at the given address, using the given options.
    fn connect(addr: SocketAddr, options: Options) -> Self::ConnectFut;
}

/// A future that resolves to a `Client` or an `io::Error`.
pub type ConnectFuture<Req, Resp, E> =
    futures::Flatten<futures::MapErr<futures::Oneshot<io::Result<Client<Req, Resp, E>>>,
                                     fn(futures::Canceled) -> io::Error>>;

impl<Req, Resp, E> ClientExt for Client<Req, Resp, E>
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

        let max_payload_size = options.max_payload_size;

        let connect = move |handle: &reactor::Handle| {
            let handle2 = handle.clone();
            TcpStream::connect(&addr, handle)
                .and_then(move |socket| {
                    // TODO(https://github.com/tokio-rs/tokio-proto/issues/132): move this into the
                    // ServerProto impl
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
                .map(move |tcp| Client::bind(&handle2, tcp, max_payload_size))
        };
        let (tx, rx) = futures::oneshot();
        let setup = move |handle: &reactor::Handle| {
            connect(handle).then(move |result| {
                // If send fails it means the client no longer cared about connecting.
                let _ = tx.send(result);
                Ok(())
            })
        };

        match options.reactor {
            Some(Reactor::Handle(handle)) => {
                handle.spawn(setup(&handle));
            }
            Some(Reactor::Remote(remote)) => {
                remote.spawn(setup);
            }
            None => {
                REMOTE.spawn(setup);
            }
        }
        fn panic(canceled: futures::Canceled) -> io::Error {
            unreachable!(canceled)
        }
        rx.map_err(panic as _).flatten()
    }
}

type ResponseFuture<Req, Resp, E> =
    futures::AndThen<futures::MapErr<
    futures::Map<<ClientService<StreamType, Proto<Req, Response<Resp, E>>> as Service>::Future,
                 fn(WireResponse<Resp, E>) -> Result<Resp, ::Error<E>>>,
        fn(io::Error) -> ::Error<E>>,
                 Result<Resp, ::Error<E>>,
                 fn(Result<Resp, ::Error<E>>) -> Result<Resp, ::Error<E>>>;

type WireResponse<R, E> = Result<Response<R, E>, bincode::Error>;
