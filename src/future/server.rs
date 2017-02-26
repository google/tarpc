// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use {bincode, net2};
use errors::WireError;
use futures::{Future, Poll, Stream, future as futures, stream};
use futures::sync::{mpsc, oneshot};
use futures::unsync;
use protocol::Proto;
use serde::{Deserialize, Serialize};
use std::cell::Cell;
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

/// A handle to a bound server.
#[derive(Clone)]
pub struct Handle {
    addr: SocketAddr,
    shutdown: Shutdown,
}

impl Handle {
    #[doc(hidden)]
    pub fn listen<S, Req, Resp, E>(new_service: S,
                                   addr: SocketAddr,
                                   handle: &reactor::Handle,
                                   options: Options)
                                   -> io::Result<(Self, Listen<S, Req, Resp, E>)>
        where S: NewService<Request = Result<Req, bincode::Error>,
                            Response = Response<Resp, E>,
                            Error = io::Error> + 'static,
              Req: Deserialize + 'static,
              Resp: Serialize + 'static,
              E: Serialize + 'static
    {
        let (addr, shutdown, server) =
            listen_with(new_service, addr, handle, Acceptor::from(options))?;
        Ok((Handle {
                addr: addr,
                shutdown: shutdown,
            },
            server))
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

enum Acceptor {
    Tcp,
    #[cfg(feature = "tls")]
    Tls(TlsAcceptor),
}

#[cfg(feature = "tls")]
type Accept = futures::Either<futures::MapErr<futures::Map<AcceptAsync<TcpStream>,
                                                           fn(TlsStream<TcpStream>) -> StreamType>,
                                              fn(native_tls::Error) -> io::Error>,
                              futures::FutureResult<StreamType, io::Error>>;

#[cfg(not(feature = "tls"))]
type Accept = futures::FutureResult<TcpStream, io::Error>;

impl Acceptor {
    #[cfg(feature = "tls")]
    fn accept(&self, socket: TcpStream) -> Accept {
        match *self {
            Acceptor::Tls(ref tls_acceptor) => {
                futures::Either::A(tls_acceptor.accept_async(socket)
                    .map(StreamType::Tls as _)
                    .map_err(native_to_io))
            }
            Acceptor::Tcp => futures::Either::B(futures::ok(StreamType::Tcp(socket))),
        }
    }

    #[cfg(not(feature = "tls"))]
    fn accept(&self, socket: TcpStream) -> Accept {
        futures::ok(socket)
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

/// A hook to shut down a running server.
#[derive(Clone)]
pub struct Shutdown {
    tx: mpsc::UnboundedSender<oneshot::Sender<()>>,
}

/// A future that resolves when server shutdown completes.
pub struct ShutdownFuture {
    inner: futures::Either<futures::FutureResult<(), ()>,
                           futures::OrElse<oneshot::Receiver<()>, Result<(), ()>, AlwaysOk>>,
}

impl Future for ShutdownFuture {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<(), ()> {
        self.inner.poll()
    }
}

impl Shutdown {
    /// Initiates an orderly server shutdown.
    ///
    /// First, the server enters lameduck mode, in which
    /// existing connections are honored but no new connections are accepted. Then, once all
    /// connections are closed, it initates total shutdown.
    ///
    /// This fn will not return until the server is completely shut down.
    pub fn shutdown(self) -> ShutdownFuture {
        let (tx, rx) = oneshot::channel();
        let inner = if let Err(_) = self.tx.send(tx) {
            trace!("Server already initiated shutdown.");
            futures::Either::A(futures::ok(()))
        } else {
            futures::Either::B(rx.or_else(AlwaysOk))
        };
        ShutdownFuture { inner: inner }
    }
}

enum ConnectionAction {
    Increment,
    Decrement,
}

#[derive(Clone)]
struct ConnectionTracker {
    tx: unsync::mpsc::UnboundedSender<ConnectionAction>,
}

impl ConnectionTracker {
    fn increment(&self) {
        let _ = self.tx.send(ConnectionAction::Increment);
    }

    fn decrement(&self) {
        debug!("Closing connection");
        let _ = self.tx.send(ConnectionAction::Decrement);
    }
}

struct ConnectionTrackingService<S> {
    service: S,
    tracker: ConnectionTracker,
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
        debug!("Dropping ConnnectionTrackingService.");
        self.tracker.decrement();
    }
}

struct ConnectionTrackingNewService<S> {
    new_service: S,
    connection_tracker: ConnectionTracker,
}

impl<S: NewService> NewService for ConnectionTrackingNewService<S> {
    type Request = S::Request;
    type Response = S::Response;
    type Error = S::Error;
    type Instance = ConnectionTrackingService<S::Instance>;

    fn new_service(&self) -> io::Result<Self::Instance> {
        self.connection_tracker.increment();
        Ok(ConnectionTrackingService {
            service: self.new_service.new_service()?,
            tracker: self.connection_tracker.clone(),
        })
    }
}

struct ShutdownSetter {
    shutdown: Rc<Cell<Option<oneshot::Sender<()>>>>,
}

impl FnOnce<(oneshot::Sender<()>,)> for ShutdownSetter {
    type Output = ();

    extern "rust-call" fn call_once(self, tx: (oneshot::Sender<()>,)) {
        self.call(tx);
    }
}

impl FnMut<(oneshot::Sender<()>,)> for ShutdownSetter {
    extern "rust-call" fn call_mut(&mut self, tx: (oneshot::Sender<()>,)) {
        self.call(tx);
    }
}

impl Fn<(oneshot::Sender<()>,)> for ShutdownSetter {
    extern "rust-call" fn call(&self, (tx,): (oneshot::Sender<()>,)) {
        debug!("Received shutdown request.");
        self.shutdown.set(Some(tx));
    }
}

struct ConnectionWatcher {
    connections: Rc<Cell<u64>>,
}

impl FnOnce<(ConnectionAction,)> for ConnectionWatcher {
    type Output = ();

    extern "rust-call" fn call_once(self, action: (ConnectionAction,)) {
        self.call(action);
    }
}

impl FnMut<(ConnectionAction,)> for ConnectionWatcher {
    extern "rust-call" fn call_mut(&mut self, action: (ConnectionAction,)) {
        self.call(action);
    }
}

impl Fn<(ConnectionAction,)> for ConnectionWatcher {
    extern "rust-call" fn call(&self, (action,): (ConnectionAction,)) {
        match action {
            ConnectionAction::Increment => self.connections.set(self.connections.get() + 1),
            ConnectionAction::Decrement => self.connections.set(self.connections.get() - 1),
        }
        trace!("Open connections: {}", self.connections.get());
    }
}

struct ShutdownPredicate {
    shutdown: Rc<Cell<Option<oneshot::Sender<()>>>>,
    connections: Rc<Cell<u64>>,
}

impl<T> FnOnce<T> for ShutdownPredicate {
    type Output = Result<bool, ()>;

    extern "rust-call" fn call_once(self, arg: T) -> Self::Output {
        self.call(arg)
    }
}

impl<T> FnMut<T> for ShutdownPredicate {
    extern "rust-call" fn call_mut(&mut self, arg: T) -> Self::Output {
        self.call(arg)
    }
}

impl<T> Fn<T> for ShutdownPredicate {
    extern "rust-call" fn call(&self, _: T) -> Self::Output {
        match self.shutdown.take() {
            Some(shutdown) => {
                let num_connections = self.connections.get();
                debug!("Lameduck mode: {} open connections", num_connections);
                if num_connections == 0 {
                    debug!("Shutting down.");
                    let _ = shutdown.complete(());
                    Ok(false)
                } else {
                    self.shutdown.set(Some(shutdown));
                    Ok(true)
                }
            }
            None => Ok(true),
        }
    }
}

struct Warn(&'static str);

impl<T> FnOnce<T> for Warn {
    type Output = ();

    extern "rust-call" fn call_once(self, arg: T) -> Self::Output {
        self.call(arg)
    }
}

impl<T> FnMut<T> for Warn {
    extern "rust-call" fn call_mut(&mut self, arg: T) -> Self::Output {
        self.call(arg)
    }
}

impl<T> Fn<T> for Warn {
    extern "rust-call" fn call(&self, _: T) -> Self::Output {
        warn!("{}", self.0)
    }
}

struct AlwaysOk;

impl<T> FnOnce<T> for AlwaysOk {
    type Output = Result<(), ()>;

    extern "rust-call" fn call_once(self, arg: T) -> Self::Output {
        self.call(arg)
    }
}

impl<T> FnMut<T> for AlwaysOk {
    extern "rust-call" fn call_mut(&mut self, arg: T) -> Self::Output {
        self.call(arg)
    }
}

impl<T> Fn<T> for AlwaysOk {
    extern "rust-call" fn call(&self, _: T) -> Self::Output {
        Ok(())
    }
}

type ShutdownStream = stream::Map<stream::Take<mpsc::UnboundedReceiver<oneshot::Sender<()>>>,
                                  ShutdownSetter>;

type ConnectionStream = stream::Map<unsync::mpsc::UnboundedReceiver<ConnectionAction>,
                                    ConnectionWatcher>;

struct ShutdownWatcher {
    inner: stream::ForEach<stream::MapErr<stream::TakeWhile<stream::Merge<ShutdownStream,
                                                                          ConnectionStream>,
                                                            ShutdownPredicate,
                                                            Result<bool, ()>>,
                                          Warn>,
                           AlwaysOk,
                           Result<(), ()>>,
}

impl Future for ShutdownWatcher {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<(), ()> {
        self.inner.poll()
    }
}

/// Creates a future that completes when a shutdown is signaled and no connections are open.
fn shutdown_watcher() -> (ConnectionTracker, Shutdown, ShutdownWatcher) {
    let (shutdown_tx, shutdown_rx) = mpsc::unbounded::<oneshot::Sender<()>>();
    let (connection_tx, connection_rx) = unsync::mpsc::unbounded();
    let shutdown = Rc::new(Cell::new(None));
    let connections = Rc::new(Cell::new(0));
    let shutdown2 = shutdown.clone();
    let connections2 = connections.clone();

    let inner = shutdown_rx.take(1)
        .map(ShutdownSetter { shutdown: shutdown })
        .merge(connection_rx.map(ConnectionWatcher { connections: connections }))
        .take_while(ShutdownPredicate {
            shutdown: shutdown2,
            connections: connections2,
        })
        .map_err(Warn("UnboundedReceiver resolved to an Err; can it do that?"))
        .for_each(AlwaysOk);

    (ConnectionTracker { tx: connection_tx },
     Shutdown { tx: shutdown_tx },
     ShutdownWatcher { inner: inner })
}

type AcceptStream = stream::AndThen<Incoming, Acceptor, Accept>;

type BindStream<S> = stream::ForEach<AcceptStream,
                                     Bind<ConnectionTrackingNewService<S>>,
                                     io::Result<()>>;

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
    inner: futures::Then<futures::Select<futures::MapErr<BindStream<S>, fn(io::Error)>,
                                         ShutdownWatcher>,
                         Result<(), ()>,
                         AlwaysOk>,
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
                                -> io::Result<(SocketAddr, Shutdown, Listen<S, Req, Resp, E>)>
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

    let (connection_tracker, shutdown, shutdown_future) = shutdown_watcher();
    let server = listener.incoming()
        .and_then(acceptor)
        .for_each(Bind {
            handle: handle,
            new_service: ConnectionTrackingNewService {
                connection_tracker: connection_tracker,
                new_service: new_service,
            },
        })
        .map_err(log_err as _);

    let server = server.select(shutdown_future).then(AlwaysOk);
    Ok((addr, shutdown, Listen { inner: server }))
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
