// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use mio::*;
use mio::tcp::{TcpListener, TcpStream};
use std::collections::{HashMap, HashSet, VecDeque};
use std::collections::hash_map::Entry;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
use ::{Error, RegisterServerError, DeregisterServerError, ShutdownServerError};
use super::{ReadState, WriteState, Packet};

/// The low-level trait implemented by services running on the tarpc event loop.
pub trait Service: Send {
    /// Handle a request `packet` directed to connection `token` running on `event_loop`.
    fn handle(&mut self,
              connection: &mut ClientConnection,
              packet: Packet,
              event_loop: &mut EventLoop<Dispatcher>);
}
impl<F> Service for F
    where F: for <'a, 'b> FnMut(&'a mut ClientConnection, Packet, &'b mut EventLoop<Dispatcher>),
          F: Send
{
    fn handle(&mut self,
              connection: &mut ClientConnection,
              packet: Packet,
              event_loop: &mut EventLoop<Dispatcher>) 
    {
        self(connection, packet, event_loop);
    }
}

/// A connection to a client. Contains in-progress reads and writes as well as pending replies.
pub struct ClientConnection {
    socket: TcpStream,
    outbound: VecDeque<Packet>,
    tx: Option<WriteState>,
    rx: ReadState,
    pub token: Token,
    interest: EventSet,
    service: Token,
}

impl ClientConnection {
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
    fn readable<S: ?Sized>(&mut self, service: &mut S, event_loop: &mut EventLoop<Dispatcher>)
        -> io::Result<()>
            where S: Service
    {
        debug!("ClientConnection {:?}: socket readable.", self.token);
        if let Some(packet) = ReadState::next(&mut self.rx,
                        &mut self.socket,
                        &mut self.interest,
                        self.token)
        {
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
        where S: Service
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
    pub fn reply(&mut self,
                 event_loop: &mut EventLoop<Dispatcher>,
                 packet: Packet) {
        self.outbound.push_back(packet);
        self.interest.insert(EventSet::writable());
        if let Err(e) = self.reregister(event_loop) {
            warn!("Couldn't register with event loop. :'( {:?}", e);
        }
    }

    #[inline]
    fn register(self,
                event_loop: &mut EventLoop<Dispatcher>,
                connections: &mut HashMap<Token, ClientConnection>) -> io::Result<()> {
        event_loop.register(&self.socket,
                            self.token,
                            self.interest,
                            PollOpt::edge() | PollOpt::oneshot())?;
        connections.insert(self.token, self);
        Ok(())
    }

    #[inline]
    fn deregister(&mut self,
                  event_loop: &mut EventLoop<Dispatcher>,
                  server: &mut Server) -> io::Result<()>
    {
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
pub struct Server {
    socket: TcpListener,
    service: Box<Service>,
    connections: HashSet<Token>
}

/// A handle to the server.
pub struct ServeHandle {
    pub local_addr: SocketAddr,
    registry: Registry,
    token: Token,
}

impl ServeHandle {
    pub fn deregister(self) -> Result<Server, Error> {
        self.registry.deregister(self.token)
    }

    pub fn shutdown(self) -> Result<(), Error> {
        self.registry.shutdown()
    }
}

impl Server {
    pub fn new<A, S>(addr: A, service: S) -> Result<Server, Error>
        where A: ToSocketAddrs,
              S: Service + 'static
    {
        let addr = if let Some(addr) = addr.to_socket_addrs()?.next() {
            addr
        } else { 
            return Err(Error::NoAddressFound)
        };
        let socket = TcpListener::bind(&addr)?;
        Ok(Server {
            socket: socket,
            service: Box::new(service),
            connections: HashSet::new()
        })
    }

    pub fn spawn<A, S>(addr: A, service: S) -> Result<ServeHandle, Error>
        where A: ToSocketAddrs,
              S: Service + 'static
    {
        let server = Server::new(addr, service)?;
        let mut event_loop = EventLoop::new().expect(pos!());
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
        debug!("Server {:?}: ready: {:?}", server_token, events);
        if events.is_readable() {
            let socket = self.socket.accept().unwrap().unwrap().0;
            let token = Token(*next_handler_id);
            info!("Server {:?}: registering ClientConnection {:?}", server_token, token);
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
                            servers: &mut HashMap<Token, Server>,
                            event_loop: &mut EventLoop<H>) -> io::Result<()>
    {
        event_loop.register(&self.socket,
                            token,
                            EventSet::readable(),
                            PollOpt::edge() | PollOpt::oneshot())?;
        servers.insert(token, self);
        Ok(())
    }

    fn reregister<H: Handler>(&self, token: Token, event_loop: &mut EventLoop<H>)
        -> io::Result<()>
    {
        event_loop.reregister(&self.socket,
                              token,
                              EventSet::readable(),
                              PollOpt::edge() | PollOpt::oneshot())
    }

    fn deregister<H: Handler>(&mut self,
                              event_loop: &mut EventLoop<H>,
                              connections: &mut HashMap<Token, ClientConnection>)
        -> io::Result<()>
    {
        for conn in self.connections.drain() {
            event_loop.deregister(&connections.remove(&conn).unwrap().socket).unwrap();
        }
        event_loop.deregister(&self.socket)
    }
}

pub struct Dispatcher {
    servers: HashMap<Token, Server>,
    connections: HashMap<Token, ClientConnection>,
    next_handler_id: usize,
}

impl Dispatcher {
    pub fn new() -> Dispatcher {
        Dispatcher {
            servers: HashMap::new(),
            connections: HashMap::new(),
            next_handler_id: 0,
        }
    }

    pub fn spawn() -> Registry {
        let mut event_loop = EventLoop::new().expect(pos!());
        let handle = event_loop.channel();
        thread::spawn(move || {
            if let Err(e) = event_loop.run(&mut Dispatcher::new()) {
                error!("Event loop failed: {:?}", e);
            }
        });
        Registry { handle: handle }
    }
}

#[derive(Clone)]
pub struct Registry {
    handle: Sender<Action>,
}

impl Registry {
    pub fn register(self, server: Server) -> Result<ServeHandle, Error> {
        let (tx, rx) = mpsc::channel();
        let addr = server.socket.local_addr()?;
        self.handle.send(Action::Register(server, tx)).map_err(|e| RegisterServerError(e))?;
        let token = rx.recv()?;
        Ok(ServeHandle {
            local_addr: addr,
            registry: self,
            token: token,
        })
    }

    pub fn deregister(&self, token: Token) -> Result<Server, Error> {
        let (tx, rx) = mpsc::channel();
        self.handle.send(Action::Deregister(token, tx)).map_err(|e| DeregisterServerError(e))?;
        Ok(rx.recv()?)
    }

    pub fn shutdown(&self) -> Result<(), Error> {
        self.handle.send(Action::Shutdown).map_err(|e| ShutdownServerError(e))?;
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
                    warn!("Dispatcher: failed to register service {:?}, {:?}", token, e);
                }
                if let Err(e) = tx.send(token) {
                    warn!("Dispatcher: failed to send registered service's token, {:?}", e);
                }
            }
            Action::Deregister(token, tx) => {
                let mut server = self.servers.remove(&token).unwrap();
                if let Err(e) = server.deregister(event_loop, &mut self.connections) {
                    warn!("Dispatcher: failed to deregister service {:?}, {:?}", token, e);
                }
                if let Err(e) = tx.send(server) {
                    warn!("Dispatcher: failed to send deregistered service's token, {:?}, {:?}",
                          token, e);
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

pub enum Action {
    Register(Server, mpsc::Sender<Token>),
    Deregister(Token, mpsc::Sender<Server>),
    Reply(Token, Packet),
    Shutdown,
}
