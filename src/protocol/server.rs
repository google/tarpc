// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use fnv::FnvHasher;
use mio::*;
use serde::Serialize;
use std::borrow::Borrow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::collections::hash_map::Entry;
use std::convert::TryInto;
use std::fmt;
use std::hash::BuildHasherDefault;
use std::io;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;
use super::{ReadState, RpcId};
use threadpool::ThreadPool;
use {CanonicalRpcError, Error, Listener, RpcError, Stream, num_cpus};

lazy_static! {
    /// The server global event loop on which all servers are registered by default.
    pub static ref REGISTRY: Registry = {
        let mut config = EventLoopConfig::default();
        config.notify_capacity(1_000_000);
        let mut event_loop = EventLoop::configured(config)
            .expect("Tarpc startup: could not configure the server global event loop!");
        let handle = event_loop.channel();
        thread::spawn(move || {
            if let Err(e) = event_loop.run(&mut Dispatcher::new()) {
                error!("Event loop failed: {:?}", e);
            }
        });
        Registry { handle: handle }
    };
}

type Packet = super::Packet<Vec<u8>>;
type WriteState = super::WriteState<Vec<u8>>;

/// The request context by which replies are sent.
#[derive(Debug)]
pub struct GenericCtx {
    request_id: RpcId,
    connection_token: Token,
    tx: Sender<Action>,
}

impl GenericCtx {
    /// The id of the request, guaranteed to be unique for the associated connection.
    #[inline]
    pub fn request_id(&self) -> RpcId {
        self.request_id
    }

    /// The token representing the connection, guaranteed to be unique across all tokens
    /// associated with the event loop the connection is running on.
    #[inline]
    pub fn connection_token(&self) -> Token {
        self.connection_token
    }

    /// Convert the context into a version that can be sent across threads.
    #[inline]
    pub fn for_type<O>(self) -> Ctx<O>
        where O: Serialize
    {
        Ctx {
            request_id: self.request_id,
            token: self.connection_token,
            tx: self.tx,
            phantom_data: PhantomData,
        }
    }
}

/// The request context by which replies are sent. Same as `Ctx` but can be sent across
/// threads.
#[derive(Clone, Debug)]
pub struct Ctx<O>
    where O: Serialize
{
    request_id: RpcId,
    token: Token,
    tx: Sender<Action>,
    phantom_data: PhantomData<O>,
}

impl<O> Ctx<O>
    where O: Serialize
{
    /// The id of the request, guaranteed to be unique for the associated connection.
    #[inline]
    pub fn request_id(&self) -> RpcId {
        self.request_id
    }

    /// The token representing the connection, guaranteed to be unique across all tokens
    /// associated with the event loop the connection is running on.
    #[inline]
    pub fn connection_token(&self) -> Token {
        self.token
    }

    /// Send a reply for the request associated with this context.
    #[inline]
    pub fn reply<_O = O, _E = RpcError>(self, result: Result<_O, _E>) -> ::Result<()>
        where _O: Borrow<O>,
              _E: Into<CanonicalRpcError>
    {

        let reply = try!(serialize_reply(self.request_id, result));
        try!(self.tx.send(Action::Reply(self.token, reply)));
        Ok(())
    }

    /// Send a successful reply for the request associated with this context.
    #[inline]
    pub fn ok<_O = O>(self, reply: _O) -> ::Result<()>
        where _O: Borrow<O>
    {

        self.reply(Ok(reply))
    }

    /// Send an unsuccessful reply for the request associated with this context.
    #[inline]
    pub fn err<_E = RpcError>(self, failure: _E) -> ::Result<()>
        where _E: Into<CanonicalRpcError>
    {
        self.reply(Err(failure))
    }

    /// Send a busy response to the client.
    #[inline]
    pub fn busy(self) -> ::Result<()> {
        self.reply(Err(Error::Busy))
    }
}

/// The low-level trait implemented by services running on the tarpc event loop.
pub trait AsyncService: Send + Sync + fmt::Debug {
    /// Handle a request.
    fn handle(&self, ctx: GenericCtx, request: Vec<u8>);
}

/// A connection to a client. Contains in-progress reads and writes as well as pending replies.
#[derive(Debug)]
struct ClientConnection {
    socket: Stream,
    outbound: VecDeque<Packet>,
    tx: Option<WriteState>,
    rx: ReadState,
    token: Token,
    interest: EventSet,
    service: Token,
}

impl ClientConnection {
    /// Get the token registered for this client.
    #[inline]
    fn token(&self) -> Token {
        self.token
    }

    /// Make a new Client.
    fn new(token: Token, service: Token, sock: Stream) -> ClientConnection {
        ClientConnection {
            socket: sock,
            outbound: VecDeque::new(),
            tx: None,
            rx: ReadState::init(),
            token: token,
            service: service,
            interest: EventSet::readable() | EventSet::hup(),
        }
    }

    #[inline]
    fn writable(&mut self, event_loop: &mut EventLoop<Dispatcher>) -> io::Result<()> {
        debug!("ClientConnection {:?}: socket writable.", self.token);
        WriteState::next(&mut self.tx,
                         &mut self.socket,
                         &mut self.outbound,
                         &mut self.interest,
                         self.token);
        self.reregister(event_loop)
    }

    #[inline]
    fn readable(&mut self,
                mut service: &mut AsyncServer,
                event_loop: &mut EventLoop<Dispatcher>,
                threads: &ThreadPool)
                -> io::Result<()> {
        debug!("ClientConnection {:?}: socket readable.", self.token);
        if let Some(packet) = ReadState::next(&mut self.rx, &mut self.socket, self.token) {
            service.active_requests += 1;
            let ctx = GenericCtx {
                request_id: packet.id,
                connection_token: self.token(),
                tx: event_loop.channel(),
            };
            if service.active_requests > service.max_requests {
                threads.execute(move || {
                    if let Err(e) = ctx.for_type::<()>().busy() {
                        error!("Serialize thread: could not send reply, {:?}", e);
                    }
                });
            } else {
                let service = service.service.clone();
                threads.execute(move || service.handle(ctx, packet.payload));
            }
        }
        self.reregister(event_loop)
    }

    #[inline]
    fn on_ready(&mut self,
                event_loop: &mut EventLoop<Dispatcher>,
                threads: &ThreadPool,
                token: Token,
                events: EventSet,
                service: &mut AsyncServer) {
        debug!("ClientConnection {:?}: ready: {:?}", token, events);
        assert_eq!(token, self.token);
        if events.is_readable() {
            self.readable(service, event_loop, threads).unwrap();
        }
        if events.is_writable() {
            self.writable(event_loop).unwrap();
        }
    }

    /// Start sending a reply packet.
    #[inline]
    fn reply(&mut self,
             active_requests: &mut u32,
             event_loop: &mut EventLoop<Dispatcher>,
             packet: super::Packet<Vec<u8>>) {
        self.outbound.push_back(packet);
        self.interest.insert(EventSet::writable());
        if let Err(e) = self.reregister(event_loop) {
            warn!("Couldn't register with event loop. :'( {:?}", e);
        }
        *active_requests -= 1;
    }

    #[inline]
    fn register(self,
                event_loop: &mut EventLoop<Dispatcher>,
                connections: &mut HashMap<Token, ClientConnection, BuildHasherDefault<FnvHasher>>)
                -> io::Result<()> {
        try!(event_loop.register(&self.socket,
                                 self.token,
                                 self.interest,
                                 PollOpt::edge() | PollOpt::oneshot()));
        connections.insert(self.token, self);
        Ok(())
    }

    #[inline]
    fn deregister(&mut self,
                  event_loop: &mut EventLoop<Dispatcher>,
                  server: &mut AsyncServer)
                  -> io::Result<()> {
        server.connections.remove(&self.token);
        event_loop.deregister(&self.socket)
    }

    #[inline]
    fn reregister(&mut self, event_loop: &mut EventLoop<Dispatcher>) -> io::Result<()> {
        event_loop.reregister(&self.socket,
                              self.token,
                              self.interest,
                              PollOpt::edge() | PollOpt::oneshot())
    }
}

/// A server is a service accepting connections on a single port.
#[derive(Debug)]
pub struct AsyncServer {
    socket: Listener,
    service: Arc<AsyncService>,
    connections: HashSet<Token>,
    active_requests: u32,
    max_requests: u32,
}

impl AsyncServer {
    /// Create a new server listening on the given address, using the given service
    /// implementation and default configuration.
    pub fn new<L, S>(addr: L, service: S) -> ::Result<AsyncServer>
        where L: TryInto<Listener, Err = Error>,
              S: AsyncService + 'static
    {
        Self::configured(addr, service, &Config::default())
    }

    /// Create a new server listening on the given address, using the given service
    /// implementation and configuration.
    pub fn configured<L, S>(addr: L, service: S, config: &Config) -> ::Result<AsyncServer>
        where L: TryInto<Listener, Err = Error>,
              S: AsyncService + 'static
    {
        let socket = try!(addr.try_into());
        Ok(AsyncServer {
            socket: socket,
            service: Arc::new(service),
            connections: HashSet::new(),
            active_requests: 0,
            max_requests: config.max_requests,
        })
    }

    /// Start a new event loop and register a new server listening on the given address and using
    /// the given service implementation.
    pub fn listen<L, S>(addr: L, service: S, config: Config) -> ::Result<ServeHandle>
        where L: TryInto<Listener, Err = Error>,
              S: AsyncService + 'static
    {
        let server = try!(AsyncServer::configured(addr, service, &config));
        config.registry.register(server)
    }

    #[inline]
    fn on_ready(&mut self,
                event_loop: &mut EventLoop<Dispatcher>,
                server_token: Token,
                events: EventSet,
                next_handler_id: &mut usize,
                connections: &mut HashMap<Token, ClientConnection, BuildHasherDefault<FnvHasher>>) {
        debug!("AsyncServer {:?}: ready: {:?}", server_token, events);
        if events.is_readable() {
            let socket = self.socket.accept().unwrap().unwrap();
            self.accept(event_loop,
                        server_token,
                        socket,
                        next_handler_id,
                        connections);
        }
        self.reregister(server_token, event_loop).unwrap();
    }

    #[inline]
    fn accept(&mut self,
              event_loop: &mut EventLoop<Dispatcher>,
              server_token: Token,
              stream: Stream,
              next_handler_id: &mut usize,
              connections: &mut HashMap<Token, ClientConnection, BuildHasherDefault<FnvHasher>>) {
        let token = Token(*next_handler_id);
        info!("AsyncServer {:?}: registering ClientConnection {:?}",
              server_token,
              token);
        *next_handler_id += 1;

        ClientConnection::new(token, server_token, stream)
            .register(event_loop, connections)
            .unwrap();
        self.connections.insert(token);
    }

    fn register(self,
                token: Token,
                services: &mut HashMap<Token, AsyncServer, BuildHasherDefault<FnvHasher>>,
                event_loop: &mut EventLoop<Dispatcher>)
                -> io::Result<()> {
        try!(event_loop.register(&self.socket,
                                 token,
                                 EventSet::readable(),
                                 PollOpt::edge() | PollOpt::oneshot()));
        services.insert(token, self);
        Ok(())
    }

    fn reregister(&self, token: Token, event_loop: &mut EventLoop<Dispatcher>) -> io::Result<()> {
        event_loop.reregister(&self.socket,
                              token,
                              EventSet::readable(),
                              PollOpt::edge() | PollOpt::oneshot())
    }

    fn deregister(&mut self,
                  event_loop: &mut EventLoop<Dispatcher>,
                  connections: &mut HashMap<Token,
                                            ClientConnection,
                                            BuildHasherDefault<FnvHasher>>)
                  -> io::Result<()> {
        for conn in self.connections.drain() {
            event_loop.deregister(&connections.remove(&conn).unwrap().socket).unwrap();
        }
        event_loop.deregister(&self.socket)
    }
}

/// Configurable server settings.
#[derive(Clone, Debug)]
pub struct Config {
    /// Maximum number of requests that can be active at any given moment. Defaults to `u32::MAX`.
    pub max_requests: u32,
    /// The registry to register with. Defaults to the global registry.
    pub registry: Registry,
    /// How long inactive threads should live for. Only applicable to services that use
    /// `CachedPool` (i.e. `SyncService`).
    pub thread_max_idle: Duration,
    /// The minimum number of threads the thread pool will contain when idle. Only applicable to
    /// services that use `CachedPool` (i.e. `SyncService`).
    pub min_threads: u32,
}

impl Config {
    /// Returns a new `Config` with maximum number of requests set to `max_requests` and all other
    /// fields default.
    pub fn max_requests(max_requests: u32) -> Config {
        Config { max_requests: max_requests, ..Config::default() }
    }

    /// Returns a new `Config` with the given `Registry` and all other
    /// fields default.
    pub fn registry(registry: Registry) -> Config {
        Config { registry: registry, ..Config::default() }
    }
}

impl Default for Config {
    fn default() -> Self {
        use std::u32;
        Config {
            max_requests: u32::MAX,
            min_threads: 0,
            registry: REGISTRY.clone(),
            // Threads live for 5 minutes by default
            thread_max_idle: Duration::from_secs(5 * 60),
        }
    }
}

/// A handle to the server.
#[derive(Clone, Debug)]
pub struct ServeHandle {
    local_addr: Option<SocketAddr>,
    registry: Registry,
    token: Token,
    count: Option<Arc<()>>,
}

impl Drop for ServeHandle {
    fn drop(&mut self) {
        info!("ServeHandle {:?}: deregistering.", self.token);
        match Arc::try_unwrap(self.count.take().unwrap()) {
            Ok(_) => {
                if let Err(e) = self.registry.deregister(self.token) {
                    error!("ServeHandle {:?}: could not deregister, {:?}",
                           self.token,
                           e);
                }
            }
            Err(count) => self.count = Some(count),
        }
    }
}

impl ServeHandle {
    /// The address the service is running on.
    #[inline]
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }

    /// Manually connects the service to the given stream.
    #[inline]
    pub fn accept<S>(&self, stream: S) -> ::Result<()>
        where S: TryInto<Stream, Err = Error>
    {
        self.registry.accept(self.token, try!(stream.try_into()))
    }
}

/// The handler running on the event loop. Handles dispatching incoming connections and requests
/// to the appropriate server running on the event loop.
pub struct Dispatcher {
    services: HashMap<Token, AsyncServer, BuildHasherDefault<FnvHasher>>,
    connections: HashMap<Token, ClientConnection, BuildHasherDefault<FnvHasher>>,
    next_handler_id: usize,
    threads: ThreadPool,
}

impl fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,
               "Dispatcher {{ services: {:?}, connections: {:?}, next_handler_id: {:?}, threads: \
                ThreadPool }}",
               self.services,
               self.connections,
               self.next_handler_id)
    }
}

impl Dispatcher {
    /// Create a new Dispatcher handling no servers or connections.
    pub fn new() -> Dispatcher {
        Dispatcher {
            services: HashMap::with_hasher(BuildHasherDefault::default()),
            connections: HashMap::with_hasher(BuildHasherDefault::default()),
            next_handler_id: 0,
            threads: ThreadPool::new(num_cpus::get()),
        }
    }

    /// Start a new event loop, returning a registry with which services can be registered.
    pub fn spawn() -> ::Result<Registry> {
        let mut config = EventLoopConfig::default();
        config.notify_capacity(1_000_000);
        let mut event_loop = try!(EventLoop::configured(config));
        let handle = event_loop.channel();
        thread::spawn(move || {
            if let Err(e) = event_loop.run(&mut Dispatcher::new()) {
                error!("Event loop failed: {:?}", e);
            }
        });
        Ok(Registry { handle: handle })
    }
}

/// The handle to the dispatcher. Sends notifications to register and deregister services, or to
/// shut down the event loop.
#[derive(Clone, Debug)]
pub struct Registry {
    handle: Sender<Action>,
}

impl Registry {
    /// Send a notificiation to the event loop to register a new service. Returns a handle to
    /// the event loop for easy deregistration.
    pub fn register(&self, server: AsyncServer) -> ::Result<ServeHandle> {
        let (tx, rx) = mpsc::channel();
        let addr = try!(server.socket.local_addr());
        try!(self.handle.send(Action::Register(server, tx)));
        let token = try!(rx.recv());
        Ok(ServeHandle {
            local_addr: addr,
            registry: self.clone(),
            token: token,
            count: Some(Arc::new(())),
        })
    }

    /// Deregister the service associated with the given `Token`.
    pub fn deregister(&self, token: Token) -> ::Result<AsyncServer> {
        let (tx, rx) = mpsc::channel();
        try!(self.handle.send(Action::Deregister(token, tx)));
        rx.recv().map_err(Error::from)
    }

    /// Manually registers a client stream with a service.
    pub fn accept(&self, token: Token, stream: Stream) -> ::Result<()> {
        self.handle.send(Action::Accept(token, stream)).map_err(Error::from)
    }

    /// Shuts down the event loop, stopping all services running on it.
    pub fn shutdown(&self) -> ::Result<()> {
        try!(self.handle.send(Action::Shutdown));
        Ok(())
    }

    /// Returns debug info about the running server Dispatcher.
    pub fn debug(&self) -> ::Result<DebugInfo> {
        let (tx, rx) = mpsc::channel();
        try!(self.handle.send(Action::Debug(tx)));
        Ok(try!(rx.recv()))
    }
}

impl Handler for Dispatcher {
    type Timeout = ();
    type Message = Action;

    #[inline]
    fn ready(&mut self, event_loop: &mut EventLoop<Self>, token: Token, events: EventSet) {
        if events.is_error() {
            warn!("Dispatcher: {:?}, {:?}, skipping.", token, events);
            return;
        } else {
            info!("Dispatcher: ready {:?}, {:?}", token, events);
        }
        if let Some(service) = self.services.get_mut(&token) {
            // Accepting a connection.
            service.on_ready(event_loop,
                             token,
                             events,
                             &mut self.next_handler_id,
                             &mut self.connections);
            return;
        }

        let mut connection = match self.connections.entry(token) {
            Entry::Occupied(connection) => connection,
            Entry::Vacant(..) => unreachable!(),
        };
        let mut service = self.services.get_mut(&connection.get().service).unwrap();
        connection.get_mut().on_ready(event_loop, &self.threads, token, events, service);
        if events.is_hup() {
            info!("ClientConnection {:?} hung up. Deregistering...", token);
            let mut connection = connection.remove();
            if let Err(e) = connection.deregister(event_loop, &mut service) {
                error!("Dispatcher: failed to deregister {:?}, {:?}", token, e);
            }
        }
    }

    #[inline]
    fn notify(&mut self, event_loop: &mut EventLoop<Self>, action: Action) {
        match action {
            Action::Accept(token, stream) => {
                // If it's not present, it must have already been deregistered.
                if let Some(server) = self.services.get_mut(&token) {
                    server.accept(event_loop,
                                  token,
                                  stream,
                                  &mut self.next_handler_id,
                                  &mut self.connections);
                }
            }
            Action::Register(server, tx) => {
                let token = Token(self.next_handler_id);
                info!("Dispatcher: registering server {:?}", token);
                self.next_handler_id += 1;
                if let Err(e) = server.register(token, &mut self.services, event_loop) {
                    warn!("Dispatcher: failed to register service {:?}, {:?}",
                          token,
                          e);
                }
                if let Err(e) = tx.send(token) {
                    warn!("Dispatcher: failed to send registered service's token, {:?}",
                          e);
                }
            }
            Action::Deregister(token, tx) => {
                // If it's not present, it must have already been deregistered.
                if let Some(mut server) = self.services.remove(&token) {
                    if let Err(e) = server.deregister(event_loop, &mut self.connections) {
                        warn!("Dispatcher: failed to deregister service {:?}, {:?}",
                              token,
                              e);
                    }
                    if let Err(e) = tx.send(server) {
                        warn!("Dispatcher: failed to send deregistered service's token, {:?}, \
                               {:?}",
                              token,
                              e);
                    }
                }
            }
            Action::Reply(token, packet) => {
                info!("Dispatcher: sending reply over connection {:?}", token);
                let cxn = self.connections.get_mut(&token).unwrap();
                let service = self.services.get_mut(&cxn.service).unwrap();
                cxn.reply(&mut service.active_requests, event_loop, packet);
            }
            Action::Shutdown => {
                info!("Shutting down event loop.");
                event_loop.shutdown();
            }
            Action::Debug(tx) => {
                if let Err(e) = tx.send(DebugInfo {
                    services: self.services.len(),
                    connections: self.connections.len(),
                    active_requests: self.services
                        .values()
                        .map(|service| service.active_requests)
                        .sum(),
                }) {
                    warn!("Dispatcher: failed to send debug info, {:?}", e);
                }
            }
        }
    }
}

/// The actions that can be requested of the `Dispatcher`.
#[derive(Debug)]
pub enum Action {
    /// Manually accept a new client connection on the specified service.
    ///
    /// This is primarily useful for clients that want to interact over transports that don't
    /// have a connection mechanism, such as Stdin/Stdout.
    Accept(Token, Stream),
    /// Register a new service.
    Register(AsyncServer, mpsc::Sender<Token>),
    /// Deregister a running service.
    Deregister(Token, mpsc::Sender<AsyncServer>),
    /// Send a reply over the connection associated with the given `Token`.
    Reply(Token, super::Packet<Vec<u8>>),
    /// Shut down the event loop.
    Shutdown,
    /// Get debug info.
    Debug(mpsc::Sender<DebugInfo>),
}

/// Serialized an rpc reply into a packet.
///
/// If the result is `Err`, first converts the error to a `CanonicalRpcError`.
#[inline]
pub fn serialize_reply<O, _O = &'static O, _E = RpcError>(request_id: RpcId,
                                                          result: Result<_O, _E>)
                                                          -> ::Result<super::Packet<Vec<u8>>>
    where O: Serialize,
          _O: Borrow<O>,
          _E: Into<CanonicalRpcError>
{
    let reply: Result<_O, CanonicalRpcError> = result.map_err(_E::into);
    let reply: Result<&O, &CanonicalRpcError> = reply.as_ref().map(_O::borrow);
    let packet = Packet {
        id: request_id,
        payload: try!(super::serialize(&reply)),
    };
    Ok(packet)
}

/// Information on the running server.
pub struct DebugInfo {
    /// Number of services managed by the dispatcher.
    pub services: usize,
    /// Number of open connections across all services.
    pub connections: usize,
    /// The number of requests that are currently being processed
    /// and which are not yet outbound.
    pub active_requests: u32,
}
