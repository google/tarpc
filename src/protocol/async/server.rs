// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::SizeLimit;
use bincode::serde::{deserialize_from, serialize};
use byteorder::{BigEndian, ByteOrder, ReadBytesExt};
use mio::*;
use mio::tcp::{TcpListener, TcpStream};
use protocol::Error;
use self::ReadState::*;
use self::WriteState::*;
use serde;
use std::collections::{HashMap, VecDeque};
use std::convert;
use std::io::{self, Cursor};
use std::marker::PhantomData;
use std::net::ToSocketAddrs;
use std::sync::mpsc;
use std::thread;

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
    fn new(token: Token, sock: TcpStream) -> ClientConnection {
        ClientConnection {
            socket: sock,
            outbound: VecDeque::new(),
            tx: None,
            rx: ReadState::init(),
            token: token,
            interest: EventSet::hup(),
        }
    }

    fn writable<H: Handler>(&mut self, event_loop: &mut EventLoop<H>) -> io::Result<()> {
        debug!("Client connection {:?}: socket writable.", self.token);
        WriteState::next(&mut self.tx,
                         &mut self.socket,
                         &mut self.outbound,
                         &mut self.interest,
                         self.token);
        self.reregister(event_loop)
    }

    fn readable<H: Handler>(&mut self, service: &mut Service, event_loop: &mut EventLoop<H>) -> io::Result<()> {
        debug!("Client connection {:?}: socket readable.", self.token);
        let inbound = &mut self.inbound;
        ReadState::next(&mut self.rx,
                        &mut self.socket,
                        |packet| service.handle(packet, event_loop),
                        &mut self.interest,
                        self.token);
        self.reregister(event_loop)
    }

    fn on_ready<H: Handler>(&mut self,
                            event_loop: &mut EventLoop<H>,
                            token: Token,
                            events: EventSet) {
        debug!("ClientConnection {:?}: ready: {:?}", token, events);
        assert_eq!(token, self.token);
        if events.is_readable() {
            self.readable(event_loop).unwrap();
        }
        if events.is_writable() {
            self.writable(event_loop).unwrap();
        }
    }

    /// Notification that a reply packet is ready to send.
    fn on_notify<H: Handler>(&mut self,
                             event_loop: &mut EventLoop<H>,
                             packet: Packet) {
        self.outbound.push_back(packet);
        self.interest.insert(EventSet::writable());
        if let Err(e) = self.reregister(event_loop) {
            warn!("Couldn't register with event loop. :'( {:?}", e);
        }
    }

    fn register<H: Handler>(&self, event_loop: &mut EventLoop<H>) -> io::Result<()> {
        event_loop.register(&self.socket,
                            self.token,
                            self.interest,
                            PollOpt::edge() | PollOpt::oneshot())
    }

    fn deregister<H: Handler>(&mut self, event_loop: &mut EventLoop<H>) -> io::Result<()> {
        event_loop.deregister(&self.socket)
    }

    fn reregister<H: Handler>(&mut self, event_loop: &mut EventLoop<H>) -> io::Result<()> {
        event_loop.reregister(&self.socket,
                              self.token,
                              self.interest,
                              PollOpt::edge() | PollOpt::oneshot())
    }
}

struct Server {
    socket: TcpListener,
    service: Box<Service>,
}

impl Server {
    fn on_ready<H: Handler>(&mut self,
                            event_loop: &mut EventLoop<H>,
                            token: Token,
                            events: EventSet) {
        debug!("ClientConnection {:?}: ready: {:?}", token, events);
        assert_eq!(token, self.token);
        if events.is_readable() {
            let sock = self.sock.accept().unwrap().unwrap().0;
            let conn = EchoConn::new(sock,);
            let tok = self.conns.insert(conn)
                .ok().expect("could not add connection to slab");

            // Register the connection
            self.conns[tok].token = Some(tok);
            event_loop.register(&self.conns[tok].sock, tok, EventSet::readable(),
                                    PollOpt::edge() | PollOpt::oneshot())
                .ok().expect("could not register socket with event loop");
        }
    }

}
trait Service {
    fn handle(&mut self, packet: Packet, event_loop: &mut EventLoop<Dispatcher>);
}

enum HandlerType {
    Server(Server),
    Connection(ClientConnection),
}

impl HandlerType {
    fn on_ready(&mut self,
                event_loop: &mut EventLoop<H>,
                token: Token,
                events: EventSet) {
        match *self {
            Server(ref mut server) => server.on_ready(event_loop, token, events),
            Connection(ref mut connection) => connection.on_ready(event_loop, token, events),
        }
    }
}

pub struct Dispatcher {
    handlers: HashMap<Token, HandlerType>,
    next_handler_id: usize,
}

impl Dispatcher {
    pub fn new() -> Dispatcher {
        Dispatcher {
            handlers: HashMap::new(),
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
    pub fn register(&self, socket: TcpStream) -> Result<ClientHandle, Error> {
        let (tx, rx) = mpsc::channel();
        self.handle.send(Action::Register(socket, tx)).map_err(|e| RegisterError(e))?;
        Ok(ClientHandle {
            token: rx.recv()?,
            register: self.clone(),
        })
    }

    pub fn deregister(&self, token: Token) -> Result<Client, Error> {
        let (tx, rx) = mpsc::channel();
        self.handle.send(Action::Deregister(token, tx)).map_err(|e| DeregisterError(e))?;
        Ok(rx.recv()?)
    }

    pub fn shutdown(&self) -> Result<(), Error> {
        self.handle.send(Action::Shutdown).map_err(|e| ShutdownError(e))?;
        Ok(())
    }
}

impl Handler for Dispatcher {
    type Timeout = ();
    type Message = Action;

    fn ready(&mut self, event_loop: &mut EventLoop<Self>, token: Token, events: EventSet) {
        self.handlers.get_mut(&token).unwrap().on_ready(event_loop, token, events);
        if events.is_hup() {
            info!("Handler {:?} socket hung up. Deregistering...", token);
            let mut client = self.connections.remove(&token).expect(pos!());
            for (_, sender) in client.inbound.drain() {
                sender.send(Err(Error::ConnectionBroken));
            }
            if let Err(e) = event_loop.deregister(&client.socket) {
                error!("Dispatcher: failed to deregister {:?}, {:?}", token, e);
            }
        }
    }

    fn notify(&mut self, event_loop: &mut EventLoop<Self>, action: Action) {
        match action {
            Action::Register(socket, tx) => {
                let token = Token(self.next_handler_id);
                self.next_handler_id += 1;
                let client = Client::new(token, socket);
                if let Err(e) = client.register(event_loop) {
                    warn!("Dispatcher: failed to register service {:?}, {:?}", token, e);
                }
                self.connections.insert(token, client);
                if let Err(e) = tx.send(token) {
                    warn!("Dispatcher: failed to send registered service's token, {:?}", e);
                }
            }
            Action::Deregister(token, tx) => {
                let mut client = self.connections.remove(&token).unwrap();
                if let Err(e) = client.deregister(event_loop) {
                    warn!("Dispatcher: failed to deregister service {:?}, {:?}", token, e);
                }
                if let Err(e) = tx.send(client) {
                    warn!("Dispatcher: failed to send deregistered service's token, {:?}, {:?}",
                          token, e);
                }
            }
            Action::Reply(token, packet) => {
                match &mut self.handlers[token] => {
                    HandlerType::Connection(ref mut conn) => {
                        conn.on_notify(event_loop, packet);
                    }
                    HandlerType::Server => unreachable!(),
                }
            }
            Action::Shutdown => {
                info!("Shutting down event loop.");
                event_loop.shutdown();
            }
        }
    }
}

pub enum Action {
    Register(Box<Service>, mpsc::Sender<Token>),
    Deregister(Token, mpsc::Sender<Box<Service>>),
    Reply(Token, Packet),
    Shutdown,
}
