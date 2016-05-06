// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use mio::*;
use mio::tcp::{TcpListener, TcpStream};
use protocol::{Error, ReadState, WriteState, RegisterServerError, DeregisterServerError, ShutdownServerError};
use protocol::client::Packet;
use std::collections::{HashMap, HashSet, VecDeque};
use std::collections::hash_map::Entry;
use std::io;
use std::sync::mpsc;
use std::thread;

pub trait Service: Send {
    fn handle(&mut self, token: Token, packet: Packet, event_loop: &mut EventLoop<Dispatcher>);
}

/// The client.
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

    fn writable(&mut self, event_loop: &mut EventLoop<Dispatcher>) -> io::Result<()> {
        debug!("Client connection {:?}: socket writable.", self.token);
        WriteState::next(&mut self.tx,
                         &mut self.socket,
                         &mut self.outbound,
                         &mut self.interest,
                         self.token);
        self.reregister(event_loop)
    }

    fn readable(&mut self, service: &mut Service, event_loop: &mut EventLoop<Dispatcher>) -> io::Result<()> {
        debug!("Client connection {:?}: socket readable.", self.token);
        let token = self.token;
        ReadState::next(&mut self.rx,
                        &mut self.socket,
                        |packet| service.handle(token, packet, event_loop),
                        &mut self.interest,
                        self.token);
        self.reregister(event_loop)
    }

    fn on_ready(&mut self,
                event_loop: &mut EventLoop<Dispatcher>,
                token: Token,
                events: EventSet,
                service: &mut Service) {
        debug!("ClientConnection {:?}: ready: {:?}", token, events);
        assert_eq!(token, self.token);
        if events.is_readable() {
            self.readable(service, event_loop).unwrap();
        }
        if events.is_writable() {
            self.writable(event_loop).unwrap();
        }
    }

    /// Notification that a reply packet is ready to send.
    fn on_notify(&mut self,
                 event_loop: &mut EventLoop<Dispatcher>,
                 packet: Packet) {
        self.outbound.push_back(packet);
        self.interest.insert(EventSet::writable());
        if let Err(e) = self.reregister(event_loop) {
            warn!("Couldn't register with event loop. :'( {:?}", e);
        }
    }

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

    fn deregister(&mut self,
                  event_loop: &mut EventLoop<Dispatcher>,
                  server: &mut Server) -> io::Result<()>
    {
        server.connections.remove(&self.token);
        event_loop.deregister(&self.socket)
    }

    fn reregister(&mut self, event_loop: &mut EventLoop<Dispatcher>) -> io::Result<()> {
        event_loop.reregister(&self.socket,
                              self.token,
                              self.interest,
                              PollOpt::edge() | PollOpt::oneshot())
    }
}

pub struct Server {
    socket: TcpListener,
    service: Box<Service>,
    connections: HashSet<Token>
}

impl Server {
    pub fn new(socket: TcpListener, service: Box<Service>) -> Server {
        Server {
            socket: socket,
            service: service,
            connections: HashSet::new()
        }
    }

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

    pub fn spawn() -> Register {
        let mut event_loop = EventLoop::new().expect(pos!());
        let handle = event_loop.channel();
        thread::spawn(move || {
            if let Err(e) = event_loop.run(&mut Dispatcher::new()) {
                error!("Event loop failed: {:?}", e);
            }
        });
        Register { handle: handle }
    }
}

#[derive(Clone)]
pub struct Register {
    handle: Sender<Action>,
}

impl Register {
    pub fn register(&self, server: Server) -> Result<Token, Error> {
        let (tx, rx) = mpsc::channel();
        self.handle.send(Action::Register(server, tx)).map_err(|e| RegisterServerError(e))?;
        Ok(rx.recv()?)
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
            info!("Client connection {:?} hung up. Deregistering...", token);
            let mut connection = connection.remove();
            if let Err(e) = connection.deregister(event_loop, &mut server) {
                error!("Dispatcher: failed to deregister {:?}, {:?}", token, e);
            }
        }
    }

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
                self.connections.get_mut(&token).unwrap().on_notify(event_loop, packet);
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
