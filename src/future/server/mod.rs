// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use {bincode, net2};
use errors::WireError;
use futures::{Async, Future, Poll, Stream, future as futures};
use protocol::Proto;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use stream_type::StreamType;
use tokio_core::net::{Incoming, TcpListener, TcpStream};
use tokio_core::reactor;
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_proto::BindServer;
use tokio_service::NewService;

mod connection;
mod shutdown;

cfg_if! {
    if #[cfg(feature = "tls")] {
        use native_tls::{self, TlsAcceptor};
        use tokio_tls::{AcceptAsync, TlsAcceptorExt, TlsStream};
        use errors::native_to_io;
    } else {}
}

pub use self::shutdown::{Shutdown, ShutdownFuture};

/// A handle to a bound server.
#[derive(Clone, Debug)]
pub struct Handle {
    addr: SocketAddr,
    shutdown: Shutdown,
}

impl Handle {
    /// Returns a hook for shutting down the server.
    pub fn shutdown(&self) -> &Shutdown {
        &self.shutdown
    }

    /// The socket address the server is bound to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

enum Acceptor {
    Tcp,
    #[cfg(feature = "tls")]
    Tls(TlsAcceptor),
}

struct Accept {
    #[cfg(feature = "tls")]
    inner: futures::Either<
        futures::MapErr<
            futures::Map<AcceptAsync<TcpStream>, fn(TlsStream<TcpStream>) -> StreamType>,
            fn(native_tls::Error) -> io::Error,
        >,
        futures::FutureResult<StreamType, io::Error>,
    >,
    #[cfg(not(feature = "tls"))]
    inner: futures::FutureResult<StreamType, io::Error>,
}

impl Future for Accept {
    type Item = StreamType;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.inner.poll()
    }
}

impl Acceptor {
    // TODO(https://github.com/tokio-rs/tokio-proto/issues/132): move this into the ServerProto impl
    #[cfg(feature = "tls")]
    fn accept(&self, socket: TcpStream) -> Accept {
        Accept {
            inner: match *self {
                Acceptor::Tls(ref tls_acceptor) => {
                    futures::Either::A(
                        tls_acceptor
                            .accept_async(socket)
                            .map(StreamType::Tls as _)
                            .map_err(native_to_io),
                    )
                }
                Acceptor::Tcp => futures::Either::B(futures::ok(StreamType::Tcp(socket))),
            },
        }
    }

    #[cfg(not(feature = "tls"))]
    fn accept(&self, socket: TcpStream) -> Accept {
        Accept {
            inner: futures::ok(StreamType::Tcp(socket)),
        }
    }
}

#[cfg(feature = "tls")]
impl From<Options> for Acceptor {
    fn from(options: Options) -> Self {
        match options.tls_acceptor {
            Some(tls_acceptor) => Acceptor::Tls(tls_acceptor),
            None => Acceptor::Tcp,
        }
    }
}

#[cfg(not(feature = "tls"))]
impl From<Options> for Acceptor {
    fn from(_: Options) -> Self {
        Acceptor::Tcp
    }
}

impl fmt::Debug for Acceptor {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        use self::Acceptor::*;
        #[cfg(feature = "tls")]
        const TLS: &'static &'static str = &"TlsAcceptor { .. }";

        match *self {
            Tcp => fmt.debug_tuple("Acceptor::Tcp").finish(),
            #[cfg(feature = "tls")]
            Tls(_) => fmt.debug_tuple("Acceptlr::Tls").field(TLS).finish(),
        }
    }
}

impl fmt::Debug for Accept {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("Accept").finish()
    }
}

#[derive(Debug)]
struct AcceptStream<S> {
    stream: S,
    acceptor: Acceptor,
    future: Option<Accept>,
}

impl<S> Stream for AcceptStream<S>
where
    S: Stream<Item = (TcpStream, SocketAddr), Error = io::Error>,
{
    type Item = <Accept as Future>::Item;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, io::Error> {
        if self.future.is_none() {
            let stream = match try_ready!(self.stream.poll()) {
                None => return Ok(Async::Ready(None)),
                Some((stream, _)) => stream,
            };
            self.future = Some(self.acceptor.accept(stream));
        }
        assert!(self.future.is_some());
        match self.future.as_mut().unwrap().poll() {
            Ok(Async::Ready(e)) => {
                self.future = None;
                Ok(Async::Ready(Some(e)))
            }
            Err(e) => {
                self.future = None;
                Err(e)
            }
            Ok(Async::NotReady) => Ok(Async::NotReady),
        }
    }
}

/// Additional options to configure how the server operates.
pub struct Options {
    /// Max packet size in bytes.
    max_payload_size: u64,
    #[cfg(feature = "tls")]
    tls_acceptor: Option<TlsAcceptor>,
}

impl Default for Options {
    #[cfg(not(feature = "tls"))]
    fn default() -> Self {
        Options {
            max_payload_size: 2 << 20,
        }
    }

    #[cfg(feature = "tls")]
    fn default() -> Self {
        Options {
            max_payload_size: 2 << 20,
            tls_acceptor: None,
        }
    }
}

impl Options {
    /// Set the max payload size in bytes. The default is 2 << 20 (2 MiB).
    pub fn max_payload_size(mut self, bytes: u64) -> Self {
        self.max_payload_size = bytes;
        self
    }

    /// Sets the `TlsAcceptor`
    #[cfg(feature = "tls")]
    pub fn tls(mut self, tls_acceptor: TlsAcceptor) -> Self {
        self.tls_acceptor = Some(tls_acceptor);
        self
    }
}

impl fmt::Debug for Options {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        #[cfg(feature = "tls")]
        const SOME: &'static &'static str = &"Some(_)";
        #[cfg(feature = "tls")]
        const NONE: &'static &'static str = &"None";

        let mut debug_struct = fmt.debug_struct("Options");
        #[cfg(feature = "tls")]
        debug_struct.field(
            "tls_acceptor",
            if self.tls_acceptor.is_some() {
                SOME
            } else {
                NONE
            },
        );
        debug_struct.finish()
    }
}

/// A message from server to client.
#[doc(hidden)]
pub type Response<T, E> = Result<T, WireError<E>>;

#[doc(hidden)]
pub fn listen<S, Req, Resp, E>(new_service: S,
                               addr: SocketAddr,
                               handle: &reactor::Handle,
                               options: Options)
                               -> io::Result<(Handle, Listen<S, Req, Resp, E>)>
    where S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: DeserializeOwned + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    let (addr, shutdown, server) = listen_with(
        new_service,
        addr,
        handle,
        options.max_payload_size,
        Acceptor::from(options),
    )?;
    Ok((
        Handle {
            addr: addr,
            shutdown: shutdown,
        },
        server,
    ))
}

/// Spawns a service that binds to the given address using the given handle.
fn listen_with<S, Req, Resp, E>(new_service: S,
                                addr: SocketAddr,
                                handle: &reactor::Handle,
                                max_payload_size: u64,
                                acceptor: Acceptor)
                                -> io::Result<(SocketAddr, Shutdown, Listen<S, Req, Resp, E>)>
    where S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: DeserializeOwned + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    let listener = listener(&addr, handle)?;
    let addr = listener.local_addr()?;
    debug!("Listening on {}.", addr);

    let handle = handle.clone();
    let (connection_tracker, shutdown, shutdown_future) = shutdown::Watcher::triple();
    let server = BindStream {
        handle: handle,
        new_service: connection::TrackingNewService {
            connection_tracker: connection_tracker,
            new_service: new_service,
        },
        stream: AcceptStream {
            stream: listener.incoming(),
            acceptor: acceptor,
            future: None,
        },
        max_payload_size: max_payload_size,
    };

    let server = AlwaysOkUnit(server.select(shutdown_future));
    Ok((addr, shutdown, Listen { inner: server }))
}

fn listener(addr: &SocketAddr, handle: &reactor::Handle) -> io::Result<TcpListener> {
    const PENDING_CONNECTION_BACKLOG: i32 = 1024;

    let builder = match *addr {
        SocketAddr::V4(_) => net2::TcpBuilder::new_v4(),
        SocketAddr::V6(_) => net2::TcpBuilder::new_v6(),
    }?;
    configure_tcp(&builder)?;
    builder.reuse_address(true)?;
    builder
        .bind(addr)?
        .listen(PENDING_CONNECTION_BACKLOG)
        .and_then(|l| TcpListener::from_listener(l, addr, handle))
}

#[cfg(unix)]
fn configure_tcp(tcp: &net2::TcpBuilder) -> io::Result<()> {
    use net2::unix::UnixTcpBuilderExt;
    tcp.reuse_port(true)?;
    Ok(())
}

#[cfg(windows)]
fn configure_tcp(_tcp: &net2::TcpBuilder) -> io::Result<()> {
    Ok(())
}

struct BindStream<S, St> {
    handle: reactor::Handle,
    new_service: connection::TrackingNewService<S>,
    stream: St,
    max_payload_size: u64,
}

impl<S, St> fmt::Debug for BindStream<S, St>
where
    S: fmt::Debug,
    St: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        const HANDLE: &'static &'static str = &"Handle { .. }";
        f.debug_struct("BindStream")
            .field("handle", HANDLE)
            .field("new_service", &self.new_service)
            .field("stream", &self.stream)
            .finish()
    }
}

impl<S, Req, Resp, E, I, St> BindStream<S, St>
    where S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: DeserializeOwned + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static,
          I: AsyncRead + AsyncWrite + 'static,
          St: Stream<Item = I, Error = io::Error>
{
    fn bind_each(&mut self) -> Poll<(), io::Error> {
        loop {
            match try!(self.stream.poll()) {
                Async::Ready(Some(socket)) => {
                    Proto::new(self.max_payload_size).bind_server(&self.handle,
                                                                  socket,
                                                                  self.new_service.new_service()?);
                }
                Async::Ready(None) => return Ok(Async::Ready(())),
                Async::NotReady => return Ok(Async::NotReady),
            }
        }
    }
}

impl<S, Req, Resp, E, I, St> Future for BindStream<S, St>
    where S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: DeserializeOwned + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static,
          I: AsyncRead + AsyncWrite + 'static,
          St: Stream<Item = I, Error = io::Error>
{
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.bind_each() {
            Ok(Async::Ready(())) => Ok(Async::Ready(())),
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(e) => {
                error!("While processing incoming connections: {}", e);
                Err(())
            }
        }
    }
}

/// The future representing a running server.
#[doc(hidden)]
pub struct Listen<S, Req, Resp, E>
    where S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: DeserializeOwned + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    inner: AlwaysOkUnit<futures::Select<BindStream<S, AcceptStream<Incoming>>, shutdown::Watcher>>,
}

impl<S, Req, Resp, E> Future for Listen<S, Req, Resp, E>
    where S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: DeserializeOwned + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<(), ()> {
        self.inner.poll()
    }
}

impl<S, Req, Resp, E> fmt::Debug for Listen<S, Req, Resp, E>
    where S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: DeserializeOwned + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        f.debug_struct("Listen").finish()
    }
}

#[derive(Debug)]
struct AlwaysOkUnit<F>(F);

impl<F> Future for AlwaysOkUnit<F>
where
    F: Future,
{
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<(), ()> {
        match self.0.poll() {
            Ok(Async::Ready(_)) | Err(_) => Ok(Async::Ready(())),
            Ok(Async::NotReady) => Ok(Async::NotReady),
        }
    }
}
