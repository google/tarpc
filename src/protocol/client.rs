// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::SizeLimit;
use bincode::serde::{deserialize_from, serialize};
use mio::*;
use mio::tcp::TcpStream;
use serde;
use std::collections::{HashMap, VecDeque};
use std::collections::hash_map::Entry;
use std::io::{self, Cursor};
use std::marker::PhantomData;
use std::net::ToSocketAddrs;
use std::sync::mpsc;
use std::thread;
use super::{ReadState, WriteState, Packet};
use ::{Error, RegisterClientError, DeregisterClientError, RpcError, ShutdownClientError};

/** Two types of ways of receiving messages from Client. */
pub enum SenderType {
    /** The nonblocking way. */
    Mio(Sender<Result<Vec<u8>, Error>>),
    /** And the blocking way. */
    Mpsc(mpsc::Sender<Result<Vec<u8>, Error>>),
}

impl SenderType {
    fn send(&self, payload: Result<Vec<u8>, Error>) {
        match *self {
            SenderType::Mpsc(ref tx) => {
                if let Err(e) = tx.send(payload) {
                    info!("Client: could not complete reply: {:?}", e);
                }
            }
            SenderType::Mio(ref tx) => {
                if let Err(e) = tx.send(payload) {
                    info!("Client: could not complete reply: {:?}", e);
                }
            }
        }
    }
}

/// A low-level client for communicating with a service. Reads and writes byte buffers. Typically
/// a type-aware client will be built on top of this.
pub struct Client {
    socket: TcpStream,
    outbound: VecDeque<Packet>,
    inbound: HashMap<u64, SenderType>,
    tx: Option<WriteState>,
    rx: ReadState,
    token: Token,
    interest: EventSet,
}

impl Client {
    fn new(token: Token, sock: TcpStream) -> Client {
        Client {
            socket: sock,
            outbound: VecDeque::new(),
            inbound: HashMap::new(),
            tx: None,
            rx: ReadState::init(),
            token: token,
            interest: EventSet::hup(),
        }
    }

    /// Starts an event loop on a thread and registers a new client connected to the given address.
    pub fn spawn<A>(addr: A) -> Result<ClientHandle, Error>
        where A: ToSocketAddrs,
    {
        let a = if let Some(a) = addr.to_socket_addrs()?.next() {
            a
        } else { 
            return Err(Error::NoAddressFound)
        };
        let sock = TcpStream::connect(&a)?;

        let register = Dispatcher::spawn();
        register.register(sock)
    }

    fn writable<H: Handler>(&mut self, event_loop: &mut EventLoop<H>) -> io::Result<()> {
        debug!("Client socket writable.");
        WriteState::next(&mut self.tx,
                         &mut self.socket,
                         &mut self.outbound,
                         &mut self.interest,
                         self.token);
        self.reregister(event_loop)
    }

    fn readable<H: Handler>(&mut self, event_loop: &mut EventLoop<H>) -> io::Result<()> {
        debug!("Client {:?}: socket readable.", self.token);
        {
            let inbound = &mut self.inbound;
            ReadState::next(&mut self.rx,
                            &mut self.socket,
                            |packet| if let Some(tx) = inbound.remove(&packet.id) {
                                tx.send(Ok(packet.payload));
                            } else {
                                warn!("Client: expected sender for id {} but got None!", packet.id);
                            },
                            &mut self.interest,
                            self.token);
        }
        self.reregister(event_loop)
    }

    fn on_ready<H: Handler>(&mut self,
                            event_loop: &mut EventLoop<H>,
                            token: Token,
                            events: EventSet) -> io::Result<()> {
        debug!("Client {:?}: ready: {:?}", token, events);
        assert_eq!(token, self.token);
        if events.is_readable() {
            try!(self.readable(event_loop));
        }
        if events.is_writable() {
            try!(self.writable(event_loop));
        }
        Ok(())
    }

    fn on_notify<H: Handler>(&mut self,
                             event_loop: &mut EventLoop<H>,
                             (id, req, sender_type): (u64, Vec<u8>, SenderType)) {
        self.outbound.push_back(Packet {
            id: id,
            payload: req,
        });
        self.inbound.insert(id, sender_type);
        self.interest.insert(EventSet::writable());
        if let Err(e) = self.reregister(event_loop) {
            warn!("Client {:?}: couldn't register with event loop, {:?}", self.token, e);
        }
    }

    fn register<H: Handler>(&self, event_loop: &mut EventLoop<H>) -> io::Result<()> {
        event_loop.register(&self.socket,
                            self.token,
                            self.interest,
                            PollOpt::edge() | PollOpt::oneshot())
    }

    fn deregister<H: Handler>(&mut self, event_loop: &mut EventLoop<H>) -> io::Result<()> {
        for (_, sender) in self.inbound.drain() {
            sender.send(Err(Error::ConnectionBroken));
        }
        event_loop.deregister(&self.socket)
    }

    fn reregister<H: Handler>(&mut self, event_loop: &mut EventLoop<H>) -> io::Result<()> {
        event_loop.reregister(&self.socket,
                              self.token,
                              self.interest,
                              PollOpt::edge() | PollOpt::oneshot())
    }
}

/// A thin wrapper around `Registry` that ensures messages are sent to the correct client.
pub struct ClientHandle
{
    token: Token,
    register: Registry,
}

impl ClientHandle
{
    /// Send a request to the server. Returns a future which can be blocked on until a reply is
    /// available.
    ///
    /// This method is generic over the request and the reply type, but typically a single client
    /// will only intend to send one type of request and receive one type of reply. This isn't
    /// encoded as type parameters in `ClientHandle` because doing so would make it harder to
    /// run multiple different clients on the same event loop.
    pub fn rpc<Req, Rep>(&self, req: &Req) -> Result<Future<Rep>, Error>
        where Req: serde::Serialize,
              Rep: serde::Deserialize,
    {
        self.register.rpc(self.token, req)
    }

    /// Deregisters the client from the event loop and returns the client.
    pub fn deregister(self) -> Result<Client, Error> {
        self.register.deregister(self.token)
    }

    /// Shuts down the underlying event loop.
    pub fn shutdown(self) -> Result<(), Error> {
        self.register.shutdown()
    }
}

impl Clone for ClientHandle {
    fn clone(&self) -> Self {
        ClientHandle {
            token: self.token,
            register: self.register.clone(),
        }
    }
}

/// An event loop handler that manages multiple clients.
pub struct Dispatcher {
    handlers: HashMap<Token, Client>,
    next_handler_id: usize,
    next_rpc_id: u64,
}

impl Dispatcher {
    fn new() -> Dispatcher {
        Dispatcher {
            handlers: HashMap::new(),
            next_handler_id: 0,
            next_rpc_id: 0,
        }
    }

    /// Starts an event loop on a thread and registers the dispatcher on it.
    ///
    /// Returns a registry, which is used to communicate with the dispatcher.
    pub fn spawn() -> Registry {
        let mut event_loop = EventLoop::new().expect("D:");
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
    /// Register a new client communicating with a service over the given socket.
    /// Returns a handle used to send commands to the client.
    pub fn register(&self, socket: TcpStream) -> Result<ClientHandle, Error> {
        let (tx, rx) = mpsc::channel();
        self.handle.send(Action::Register(socket, tx)).map_err(|e| RegisterClientError(e))?;
        Ok(ClientHandle {
            token: rx.recv()?,
            register: self.clone(),
        })
    }

    /// Deregisters a client from the event loop.
    pub fn deregister(&self, token: Token) -> Result<Client, Error> {
        let (tx, rx) = mpsc::channel();
        self.handle.send(Action::Deregister(token, tx)).map_err(|e| DeregisterClientError(e))?;
        Ok(rx.recv()?)
    }

    /// Tells the client identified by `token` to send the given request, `req`.
    pub fn rpc<Req, Rep>(&self, token: Token, req: &Req) -> Result<Future<Rep>, Error>
        where Req: serde::Serialize,
              Rep: serde::Deserialize
    {
        let (tx, rx) = mpsc::channel();
        self.handle.send(Action::Rpc(token,
                                     serialize(req, SizeLimit::Infinite).unwrap(),
                                     SenderType::Mpsc(tx))).map_err(|e| RpcError(e))?;
        Ok(Future {
            rx: rx,
            phantom_data: PhantomData,
        })
    }

    /// Shuts down the underlying event loop.
    pub fn shutdown(&self) -> Result<(), Error> {
        self.handle.send(Action::Shutdown).map_err(|e| ShutdownClientError(e))?;
        Ok(())
    }
}

/// A thin wrapper around a `mpsc::Receiver` that handles the deserialization of server replies.
#[derive(Debug)]
pub struct Future<T: serde::Deserialize> {
    rx: mpsc::Receiver<Result<Vec<u8>, Error>>,
    phantom_data: PhantomData<T>,
}

impl<T: serde::Deserialize> Future<T> {
    /// Consumes the future, blocking until the server reply is available.
    pub fn get(self) -> Result<T, Error> {
        Ok(deserialize_from::<_, T>(&mut Cursor::new(self.rx.recv()??),
                                         SizeLimit::Infinite)?)
    }
}

impl Handler for Dispatcher {
    type Timeout = ();
    type Message = Action;

    fn ready(&mut self, event_loop: &mut EventLoop<Self>, token: Token, events: EventSet) {
        let mut client = match self.handlers.entry(token) {
            Entry::Occupied(client) => client,
            Entry::Vacant(..) => unreachable!()
        };
        if let Err(e) = client.get_mut().on_ready(event_loop, token, events) {
            error!("Handler::on_ready failed for {:?}, {:?}", token, e);
        }
        if events.is_hup() {
            info!("Handler {:?} socket hung up. Deregistering...", token);
            let mut client = client.remove();
            if let Err(e) = client.deregister(event_loop) {
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
                    error!("Dispatcher: failed to register client {:?}, {:?}", token, e);
                }
                self.handlers.insert(token, client);
                if let Err(e) = tx.send(token) {
                    error!("Dispatcher: failed to send new client's token, {:?}", e);
                }
            }
            Action::Deregister(token, tx) => {
                let mut client = self.handlers.remove(&token).unwrap();
                if let Err(e) = client.deregister(event_loop) {
                    error!("Dispatcher: failed to deregister client {:?}, {:?}", token, e);
                }
                if let Err(e) = tx.send(client) {
                    error!("Dispatcher: failed to send deregistered client {:?}, {:?}", token, e);
                }
            }
            Action::Rpc(token, payload, tx) => {
                let id = self.next_rpc_id;
                self.next_rpc_id += 1;
                match self.handlers.get_mut(&token) {
                    Some(handler) => handler.on_notify(event_loop, (id, payload, tx)),
                    None => tx.send(Err(Error::ConnectionBroken)),
                }
            }
            Action::Shutdown => {
                info!("Shutting down event loop.");
                event_loop.shutdown();
            }
        }
    }
}

/// The actions that can be requested of a client. Typically users will not use `Action` directly,
/// but it is made public so that it can be examined if any errors occur when clients attempt
/// actions.
pub enum Action {
    Register(TcpStream, mpsc::Sender<Token>),
    Deregister(Token, mpsc::Sender<Client>),
    Rpc(Token, Vec<u8>, SenderType),
    Shutdown,
}
