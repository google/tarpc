// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use mio::*;
use mio::tcp::{TcpListener, TcpStream};
use serde::Serialize;
use std::borrow::Borrow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::collections::hash_map::Entry;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
use Error;
use super::{Packet, ReadState, WriteState};
use {CanonicalRpcError, RpcError};

/// The low-level trait implemented by services running on the tarpc event loop.
pub trait AsyncService: Send {
    /// Handle a request `packet` directed to connection `token` running on `event_loop`.
    fn handle(&mut self,
              connection: &mut ClientConnection,
              packet: Packet,
              event_loop: &mut EventLoop<Dispatcher>);
}

/// A connection to a client. Contains in-progress reads and writes as well as pending replies.
pub struct ClientConnection {
    socket: TcpStream,
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
    pub fn token(&self) -> Token {
        self.token
    }

    /// Make a new Client.
    fn new(token: Token, service: Token, sock: TcpStream) -> ClientConnection {
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
    fn readable<S: ?Sized>(&mut self,
                           service: &mut S,
                           event_loop: &mut EventLoop<Dispatcher>)
                           -> io::Result<()>
        where S: AsyncService
    {
        debug!("ClientConnection {:?}: socket readable.", self.token);
        if let Some(packet) = ReadState::next(&mut self.rx, &mut self.socket, self.token) {
            service.handle(self, packet, event_loop)
        }
        self.reregister(event_loop)
    }

    #[inline]
    fn on_ready<S: ?Sized>(&mut self,
                           event_loop: &mut EventLoop<Dispatcher>,
                           token: Token,
                           events: EventSet,
                           service: &mut S)
        where S: AsyncService
    {
        debug!("ClientConnection {:?}: ready: {:?}", token, events);
        assert_eq!(token, self.token);
        if events.is_readable() {
            self.readable(service, event_loop).unwrap();
        }
        if events.is_writable() {
            self.writable(event_loop).unwrap();
        }
    }

    /// Start sending a reply packet.
    #[inline]
    pub fn reply(&mut self, event_loop: &mut EventLoop<Dispatcher>, packet: Packet) {
        self.outbound.push_back(packet);
        self.interest.insert(EventSet::writable());
        if let Err(e) = self.reregister(event_loop) {
            warn!("Couldn't register with event loop. :'( {:?}", e);
        }
    }

    #[inline]
    /// Convert an rpc reply into a packet and send it to the client.
    pub fn serialize_reply<O, _O, _E>(&mut self,
                                      request_id: u64,
                                      result: Result<_O, _E>,
                                      event_loop: &mut EventLoop<Dispatcher>)
                                      -> ::Result<()>
        where O: Serialize,
              _O: Borrow<O>,
              _E: Into<CanonicalRpcError>
    {
        let packet = try!(serialize_reply(request_id, result));
        self.reply(event_loop, packet);
        Ok(())
    }

    #[inline]
    fn register(self,
                event_loop: &mut EventLoop<Dispatcher>,
                connections: &mut HashMap<Token, ClientConnection>)
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
pub struct AsyncServer {
    socket: TcpListener,
    service: Box<AsyncService>,
    connections: HashSet<Token>,
}

/// A handle to the server.
pub struct ServeHandle {
    local_addr: SocketAddr,
    registry: Registry,
    token: Token,
}

impl ServeHandle {
    /// The address the service is running on.
    #[inline]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Deregister the service so that it is no longer running.
    #[inline]
    pub fn deregister(self) -> Result<AsyncServer, Error> {
        self.registry.deregister(self.token)
    }

    /// Shut down the event loop this service is running on, stopping all other services running
    /// on the event loop.
    #[inline]
    pub fn shutdown(self) -> Result<(), Error> {
        self.registry.shutdown()
    }
}

impl AsyncServer {
    /// Create a new server listening on the given address and using the given service
    /// implementation.
    pub fn new<A, S>(addr: A, service: S) -> Result<AsyncServer, Error>
        where A: ToSocketAddrs,
              S: AsyncService + 'static
    {
        let addr = if let Some(addr) = try!(addr.to_socket_addrs()).next() {
            addr
        } else {
            return Err(Error::NoAddressFound);
        };
        let socket = try!(TcpListener::bind(&addr));
        Ok(AsyncServer {
            socket: socket,
            service: Box::new(service),
            connections: HashSet::new(),
        })
    }

    /// Start a new event loop and register a new server listening on the given address and using
    /// the given service implementation.
    pub fn spawn<A, S>(addr: A, service: S) -> Result<ServeHandle, Error>
        where A: ToSocketAddrs,
              S: AsyncService + 'static
    {
        let server = try!(AsyncServer::new(addr, service));
        let mut config = EventLoopConfig::default();
        config.notify_capacity(1_000_000);
        let mut event_loop = try!(EventLoop::configured(config));
        let handle = event_loop.channel();
        thread::spawn(move || {
            if let Err(e) = event_loop.run(&mut Dispatcher::new()) {
                error!("Event loop failed: {:?}", e);
            }
        });
        let registry = Registry { handle: handle };
        registry.register(server)
    }

    #[inline]
    fn on_ready(&mut self,
                event_loop: &mut EventLoop<Dispatcher>,
                server_token: Token,
                events: EventSet,
                next_handler_id: &mut usize,
                connections: &mut HashMap<Token, ClientConnection>) {
        debug!("AsyncServer {:?}: ready: {:?}", server_token, events);
        if events.is_readable() {
            let socket = self.socket.accept().unwrap().unwrap().0;
            let token = Token(*next_handler_id);
            info!("AsyncServer {:?}: registering ClientConnection {:?}",
                  server_token,
                  token);
            *next_handler_id += 1;

            ClientConnection::new(token, server_token, socket)
                .register(event_loop, connections)
                .unwrap();
            self.connections.insert(token);
        }
        self.reregister(server_token, event_loop).unwrap();
    }

    fn register<H: Handler>(self,
                            token: Token,
                            servers: &mut HashMap<Token, AsyncServer>,
                            event_loop: &mut EventLoop<H>)
                            -> io::Result<()> {
        try!(event_loop.register(&self.socket,
                                 token,
                                 EventSet::readable(),
                                 PollOpt::edge() | PollOpt::oneshot()));
        servers.insert(token, self);
        Ok(())
    }

    fn reregister<H: Handler>(&self,
                              token: Token,
                              event_loop: &mut EventLoop<H>)
                              -> io::Result<()> {
        event_loop.reregister(&self.socket,
                              token,
                              EventSet::readable(),
                              PollOpt::edge() | PollOpt::oneshot())
    }

    fn deregister<H: Handler>(&mut self,
                              event_loop: &mut EventLoop<H>,
                              connections: &mut HashMap<Token, ClientConnection>)
                              -> io::Result<()> {
        for conn in self.connections.drain() {
            event_loop.deregister(&connections.remove(&conn).unwrap().socket).unwrap();
        }
        event_loop.deregister(&self.socket)
    }
}

/// The handler running on the event loop. Handles dispatching incoming connections and requests
/// to the appropriate server running on the event loop.
pub struct Dispatcher {
    servers: HashMap<Token, AsyncServer>,
    connections: HashMap<Token, ClientConnection>,
    next_handler_id: usize,
}

impl Dispatcher {
    /// Create a new Dispatcher handling no servers or connections.
    pub fn new() -> Dispatcher {
        Dispatcher {
            servers: HashMap::new(),
            connections: HashMap::new(),
            next_handler_id: 0,
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
#[derive(Clone)]
pub struct Registry {
    handle: Sender<Action>,
}

impl Registry {
    /// Send a notificiation to the event loop to register a new service. Returns a handle to
    /// the event loop for easy deregistration.
    pub fn register(self, server: AsyncServer) -> Result<ServeHandle, Error> {
        let (tx, rx) = mpsc::channel();
        let addr = try!(server.socket.local_addr());
        try!(self.handle.send(Action::Register(server, tx)));
        let token = try!(rx.recv());
        Ok(ServeHandle {
            local_addr: addr,
            registry: self,
            token: token,
        })
    }

    /// Deregister the service associated with the given `Token`.
    pub fn deregister(&self, token: Token) -> Result<AsyncServer, Error> {
        let (tx, rx) = mpsc::channel();
        try!(self.handle.send(Action::Deregister(token, tx)));
        rx.recv().map_err(Error::from)
    }

    /// Shuts down the event loop, stopping all services running on it.
    pub fn shutdown(&self) -> Result<(), Error> {
        try!(self.handle.send(Action::Shutdown));
        Ok(())
    }
}

impl Handler for Dispatcher {
    type Timeout = ();
    type Message = Action;

    #[inline]
    fn ready(&mut self, event_loop: &mut EventLoop<Self>, token: Token, events: EventSet) {
        info!("Dispatcher: ready {:?}, {:?}", token, events);
        if let Some(server) = self.servers.get_mut(&token) {
            server.on_ready(event_loop,
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
        let mut server = self.servers.get_mut(&connection.get().service).unwrap();
        connection.get_mut().on_ready(event_loop, token, events, &mut *server.service);
        if events.is_hup() {
            info!("ClientConnection {:?} hung up. Deregistering...", token);
            let mut connection = connection.remove();
            if let Err(e) = connection.deregister(event_loop, &mut server) {
                error!("Dispatcher: failed to deregister {:?}, {:?}", token, e);
            }
        }
    }

    #[inline]
    fn notify(&mut self, event_loop: &mut EventLoop<Self>, action: Action) {
        match action {
            Action::Register(server, tx) => {
                let token = Token(self.next_handler_id);
                info!("Dispatcher: registering server {:?}", token);
                self.next_handler_id += 1;
                if let Err(e) = server.register(token, &mut self.servers, event_loop) {
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
                let mut server = self.servers.remove(&token).unwrap();
                if let Err(e) = server.deregister(event_loop, &mut self.connections) {
                    warn!("Dispatcher: failed to deregister service {:?}, {:?}",
                          token,
                          e);
                }
                if let Err(e) = tx.send(server) {
                    warn!("Dispatcher: failed to send deregistered service's token, {:?}, {:?}",
                          token,
                          e);
                }
            }
            Action::Reply(token, packet) => {
                info!("Dispatche: sending reply over connection {:?}", token);
                self.connections.get_mut(&token).unwrap().reply(event_loop, packet);
            }
            Action::Shutdown => {
                info!("Shutting down event loop.");
                event_loop.shutdown();
            }
        }
    }
}

/// The actions that can be requested of the `Dispatcher`.
pub enum Action {
    /// Register a new service.
    Register(AsyncServer, mpsc::Sender<Token>),
    /// Deregister a running service.
    Deregister(Token, mpsc::Sender<AsyncServer>),
    /// Send a reply over the connection associated with the given `Token`.
    Reply(Token, Packet),
    /// Shut down the event loop.
    Shutdown,
}

/// Serialized an rpc reply into a packet.
///
/// If the result is `Err`, first converts the error to a `CanonicalRpcError`.
#[inline]
pub fn serialize_reply<O, _O = &'static O, _E = RpcError>(request_id: u64,
                                                          result: Result<_O, _E>)
                                                          -> ::Result<Packet>
    where O: Serialize,
          _O: Borrow<O>,
          _E: Into<CanonicalRpcError>
{
    let reply: Result<_O, CanonicalRpcError> = result.map_err(|e| e.into());
    let reply: Result<&O, &CanonicalRpcError> = reply.as_ref().map(|o| o.borrow());
    let packet = Packet {
        id: request_id,
        payload: try!(super::serialize(&reply)),
    };
    Ok(packet)
}

/// An extension trait for `mio::Sender` that serializes replies to packets.
pub trait SenderExt {
    /// Serializes an rpc reply into a packet, then sends it to the client connection to reply
    /// with.
    fn reply<O, _O = &'static O, _E = RpcError>(&self,
                                                token: Token,
                                                request_id: u64,
                                                result: Result<_O, _E>)
                                                -> ::Result<()>
        where O: Serialize,
              _O: Borrow<O>,
              _E: Into<CanonicalRpcError>;
}

impl SenderExt for Sender<Action> {
    fn reply<O, _O = &'static O, _E = RpcError>(&self,
                                                token: Token,
                                                request_id: u64,
                                                result: Result<_O, _E>)
                                                -> ::Result<()>
        where O: Serialize,
              _O: Borrow<O>,
              _E: Into<CanonicalRpcError>
    {

        let reply = try!(serialize_reply(request_id, result));
        try!(self.send(Action::Reply(token, reply)));
        Ok(())
    }
}
