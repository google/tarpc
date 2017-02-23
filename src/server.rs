// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode;
use errors::WireError;
use futures::{Future, Poll, Stream, future, stream};
use futures::sync::mpsc;
use net2;
use protocol::Proto;
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::io;
use std::net::SocketAddr;
use std::rc::Rc;
use tokio_core::io::Io;
use tokio_core::net::{Incoming, TcpListener, TcpStream};
use tokio_core::reactor;
use tokio_proto::BindServer;
use tokio_service::{NewService, Service};

cfg_if! {
    if #[cfg(feature = "tls")] {
        use native_tls::{self, TlsAcceptor};
        use tokio_tls::{AcceptAsync, TlsAcceptorExt, TlsStream};
        use errors::native_to_io;
        use stream_type::StreamType;
    } else {}
}

enum Acceptor {
    Tcp,
    #[cfg(feature = "tls")]
    Tls(TlsAcceptor),
}

#[cfg(feature = "tls")]
type Accept = future::Either<future::MapErr<future::Map<AcceptAsync<TcpStream>,
                                                        fn(TlsStream<TcpStream>) -> StreamType>,
                                            fn(native_tls::Error) -> io::Error>,
                             future::FutureResult<StreamType, io::Error>>;

#[cfg(not(feature = "tls"))]
type Accept = future::FutureResult<TcpStream, io::Error>;

impl Acceptor {
    #[cfg(feature = "tls")]
    fn accept(&self, socket: TcpStream) -> Accept {
        match *self {
            Acceptor::Tls(ref tls_acceptor) => {
                future::Either::A(tls_acceptor.accept_async(socket)
                    .map(StreamType::Tls as _)
                    .map_err(native_to_io))
            }
            Acceptor::Tcp => future::Either::B(future::ok(StreamType::Tcp(socket))),
        }
    }

    #[cfg(not(feature = "tls"))]
    fn accept(&self, socket: TcpStream) -> Accept {
        future::ok(socket)
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

impl FnOnce<((TcpStream, SocketAddr),)> for Acceptor {
    type Output = Accept;

    extern "rust-call" fn call_once(self, ((socket, _),): ((TcpStream, SocketAddr),)) -> Accept {
        self.accept(socket)
    }
}

impl FnMut<((TcpStream, SocketAddr),)> for Acceptor {
    extern "rust-call" fn call_mut(&mut self,
                                   ((socket, _),): ((TcpStream, SocketAddr),))
                                   -> Accept {
        self.accept(socket)
    }
}

impl Fn<((TcpStream, SocketAddr),)> for Acceptor {
    extern "rust-call" fn call(&self, ((socket, _),): ((TcpStream, SocketAddr),)) -> Accept {
        self.accept(socket)
    }
}

/// Additional options to configure how the server operates.
#[derive(Default)]
pub struct Options {
    #[cfg(feature = "tls")]
    tls_acceptor: Option<TlsAcceptor>,
}

impl Options {
    /// Set the `TlsAcceptor`
    #[cfg(feature = "tls")]
    pub fn tls(mut self, tls_acceptor: TlsAcceptor) -> Self {
        self.tls_acceptor = Some(tls_acceptor);
        self
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
                               -> io::Result<(SocketAddr, Listen<S, Req, Resp, E>)>
    where S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: Deserialize + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    listen_with(new_service, addr, handle, Acceptor::from(options))
}

/// A handle to a bound server. Must be run to start serving requests.
#[must_use = "A server does nothing until `run` is called."]
pub struct Handle {
    reactor: reactor::Core,
    addr: SocketAddr,
    shutdown: Shutdown,
    server: Box<Future<Item=(), Error=()>>,
}

/// A hook to shut down a running server.
#[derive(Clone)]
pub struct Shutdown {
    tx: mpsc::UnboundedSender<::std::sync::mpsc::Sender<()>>,
}

impl Shutdown {
    /// Initiates an orderly server shutdown.
    ///
    /// First, the server enters lameduck mode, in which
    /// existing connections are honored but no new connections are accepted. Then, once all
    /// connections are closed, it initates total shutdown.
    ///
    /// This fn will not return until the server is completely shut down.
    pub fn shutdown(self) {
        let (tx, rx) = ::std::sync::mpsc::channel();
        if let Err(_) = self.tx.send(tx) {
            trace!("Server already initiated shutdown.");
            return;
        }
        trace!("Waiting for shutdown to complete...");
        match rx.recv() {
            Ok(()) => trace!("Server shutdown complete."),
            Err(e) => trace!("Server already initiated shutdown."),
        }
    }
}

struct ConnectionTrackingService<S> {
    service: S,
    tx: mpsc::UnboundedSender<ConnectionAction>,
}

impl<S: Service> Service for ConnectionTrackingService<S> {
    type Request = S::Request;
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn call(&self, req: Self::Request) -> Self::Future {
        trace!("Calling service.");
        self.service.call(req)
    }
}

impl<S> Drop for ConnectionTrackingService<S> {
    fn drop(&mut self) {
        let _ = self.tx.send(ConnectionAction::Dec);
    }
}

enum ConnectionAction { Inc, Dec, }


impl Handle {
    #[doc(hidden)]
    pub fn listen<S, Req, Resp, E>(new_service: S,
                                   addr: SocketAddr,
                                   options: Options)
                                   -> io::Result<Self>
        where S: NewService<Request = Result<Req, bincode::Error>,
                            Response = Response<Resp, E>,
                            Error = io::Error> + 'static,
              Req: Deserialize + 'static,
              Resp: Serialize + 'static,
              E: Serialize + 'static
    {
        let reactor = reactor::Core::new()?;

        let (shutdown_tx, shutdown_rx) = mpsc::unbounded::<::std::sync::mpsc::Sender<()>>();
        let (connection_tx, connection_rx) = mpsc::unbounded();
        let shutdown = Rc::new(RefCell::new(None));
        let connections = Rc::new(Cell::new(0));

        let (addr, server) = {
            let tx = connection_tx.clone();
            listen(move || {
                let _ = tx.send(ConnectionAction::Inc);
                Ok(ConnectionTrackingService {
                    service: new_service.new_service()?,
                    tx: tx.clone(),
                })
            }, addr, &reactor.handle(), options)?
        };
        let shutdown2 = shutdown.clone();
        let connections2 = connections.clone();
        let shutdown = {
            shutdown_rx
                .take(1)
                .map(move |tx| {
                    debug!("Received shutdown request.");
                    *shutdown.borrow_mut() = Some(tx)
                })
                .merge(connection_rx.map(move |action| {
                    match action {
                        ConnectionAction::Inc => connections.set(connections.get() + 1),
                        ConnectionAction::Dec => connections.set(connections.get() - 1),
                    }
                }))
                .take_while(move |_| {
                    let shutdown = shutdown2.borrow();
                    let should_continue = shutdown.is_none() || connections2.get() > 0;
                    if !should_continue {
                        debug!("Shutting down.");
                        if let Some(ref shutdown) = *shutdown {
                            let _ = shutdown.send(());
                        }
                    }
                    Ok(should_continue)
                })
                .map_err(|_| warn!("UnboundedReceiver resolved to an Err; can it do that?"))
                .for_each(|_| Ok(()))
        };
        let server = Box::new(server.select(shutdown).then(|_| Ok(())));
        let shutdown = Shutdown { tx: shutdown_tx };
        Ok(Handle { reactor, addr, shutdown, server, })
    }

    /// Runs the server on the current thread, blocking indefinitely.
    pub fn run(mut self) {
        trace!("Running...");
        match self.reactor.run(self.server) {
            Ok(()) => debug!("Server successfully shutdown."),
            Err(()) => debug!("Server shutdown due to error."),
        }
    }

    /// Returns a hook for shutting down the server.
    pub fn shutdown(&self) -> Shutdown {
        self.shutdown.clone()
    }

    /// The socket address the server is bound to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

/// The future representing a running server.
#[doc(hidden)]
pub struct Listen<S, Req, Resp, E>
    where S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: Deserialize + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    inner: future::MapErr<stream::ForEach<stream::AndThen<Incoming, Acceptor, Accept>,
                                          Bind<S>,
                                          io::Result<()>>,
                          fn(io::Error)>,
}

impl<S, Req, Resp, E> Future for Listen<S, Req, Resp, E>
    where S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: Deserialize + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<(), ()> {
        self.inner.poll()
    }
}

/// Spawns a service that binds to the given address using the given handle.
fn listen_with<S, Req, Resp, E>(new_service: S,
                                addr: SocketAddr,
                                handle: &reactor::Handle,
                                acceptor: Acceptor)
                                -> io::Result<(SocketAddr, Listen<S, Req, Resp, E>)>
    where S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: Deserialize + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    let listener = listener(&addr, handle)?;
    let addr = listener.local_addr()?;
    debug!("Listening on {}.", addr);

    let handle = handle.clone();

    let inner = listener.incoming()
        .and_then(acceptor)
        .for_each(Bind {
            handle: handle,
            new_service: new_service,
        })
        .map_err(log_err as _);
    Ok((addr, Listen { inner: inner }))
}

fn log_err(e: io::Error) {
    error!("While processing incoming connections: {}", e);
}

struct Bind<S> {
    handle: reactor::Handle,
    new_service: S,
}

impl<S, Req, Resp, E> Bind<S>
    where S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: Deserialize + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    fn bind<I>(&self, socket: I) -> io::Result<()>
        where I: Io + 'static
    {
        Proto::new().bind_server(&self.handle, socket, self.new_service.new_service()?);
        Ok(())
    }
}

impl<I, S, Req, Resp, E> FnOnce<(I,)> for Bind<S>
    where I: Io + 'static,
          S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: Deserialize + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    type Output = io::Result<()>;

    extern "rust-call" fn call_once(self, (socket,): (I,)) -> io::Result<()> {
        self.bind(socket)
    }
}

impl<I, S, Req, Resp, E> FnMut<(I,)> for Bind<S>
    where I: Io + 'static,
          S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: Deserialize + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    extern "rust-call" fn call_mut(&mut self, (socket,): (I,)) -> io::Result<()> {
        self.bind(socket)
    }
}

impl<I, S, Req, Resp, E> Fn<(I,)> for Bind<S>
    where I: Io + 'static,
          S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: Deserialize + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    extern "rust-call" fn call(&self, (socket,): (I,)) -> io::Result<()> {
        self.bind(socket)
    }
}

fn listener(addr: &SocketAddr, handle: &reactor::Handle) -> io::Result<TcpListener> {
    const PENDING_CONNECTION_BACKLOG: i32 = 1024;
    #[cfg(unix)]
    use net2::unix::UnixTcpBuilderExt;

    let builder = match *addr {
        SocketAddr::V4(_) => net2::TcpBuilder::new_v4(),
        SocketAddr::V6(_) => net2::TcpBuilder::new_v6(),
    }?;

    builder.reuse_address(true)?;

    #[cfg(unix)]
    builder.reuse_port(true)?;

    builder.bind(addr)?
        .listen(PENDING_CONNECTION_BACKLOG)
        .and_then(|l| TcpListener::from_listener(l, addr, handle))
}
