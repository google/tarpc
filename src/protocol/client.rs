// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use mio::*;
use mio::tcp::TcpStream;
use serde;
use std::collections::{HashMap, VecDeque};
use std::collections::hash_map::Entry;
use std::io;
use std::marker::PhantomData;
use std::net::ToSocketAddrs;
use std::sync::mpsc;
use std::thread;
use super::{Packet, ReadState, WriteState, deserialize, serialize};
use Error;

/// Two types of ways of receiving messages from Client.
pub enum SenderType {
    /// The nonblocking way.
    Mio(Sender<::Result<Vec<u8>>>),
    /// And the blocking way.
    Mpsc(mpsc::Sender<::Result<Vec<u8>>>),
}

impl SenderType {
    fn send(&self, payload: ::Result<Vec<u8>>) {
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

/// A function called when the rpc reply is available.
pub trait ReplyCallback {
    /// Consumes the rpc result.
    fn accept(self: Box<Self>, result: ::Result<Vec<u8>>);
}

impl<F> ReplyCallback for F
    where F: FnOnce(::Result<Vec<u8>>)
{
    fn accept(self: Box<Self>, result: ::Result<Vec<u8>>) {
        self(result)
    }
}

/// The types of ways a client can be notified that their rpc is complete.
pub enum ReplyHandler {
    /// Send the reply over a channel.
    Sender(SenderType),
    /// Call the callback with the reply as argument.
    Callback(Box<ReplyCallback + Send>),
}

impl ReplyHandler {
    fn handle(self, payload: ::Result<Vec<u8>>) {
        match self {
            ReplyHandler::Sender(sender) => sender.send(payload),
            ReplyHandler::Callback(cb) => cb.accept(payload),
        }
    }
}

/// A low-level client for communicating with a service. Reads and writes byte buffers. Typically
/// a type-aware client will be built on top of this.
pub struct Client {
    socket: TcpStream,
    outbound: VecDeque<Packet>,
    inbound: HashMap<u64, ReplyHandler>,
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
    pub fn spawn<A>(addr: A) -> ::Result<ClientHandle>
        where A: ToSocketAddrs
    {
        try!(Dispatcher::spawn()).register(addr)
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
            if let Some(packet) = ReadState::next(&mut self.rx,
                                                  &mut self.socket,
                                                  &mut self.interest,
                                                  self.token) {
                if let Some(tx) = inbound.remove(&packet.id) {
                    tx.handle(Ok(packet.payload));
                } else {
                    warn!("Client: expected sender for id {} but got None!", packet.id);
                }
            }
        }
        self.reregister(event_loop)
    }

    fn on_ready<H: Handler>(&mut self,
                            event_loop: &mut EventLoop<H>,
                            token: Token,
                            events: EventSet)
                            -> io::Result<()> {
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
                             (id, req, handler): (u64, Vec<u8>, ReplyHandler)) {
        self.outbound.push_back(Packet {
            id: id,
            payload: req,
        });
        self.inbound.insert(id, handler);
        self.interest.insert(EventSet::writable());
        if let Err(e) = self.reregister(event_loop) {
            warn!("Client {:?}: couldn't register with event loop, {:?}",
                  self.token,
                  e);
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
            sender.handle(Err(Error::ConnectionBroken));
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
pub struct ClientHandle {
    token: Token,
    register: Registry,
}

impl ClientHandle {
    /// Send a request to the server. Returns a future which can be blocked on until a reply is
    /// available.
    ///
    /// This method is generic over the request and the reply type, but typically a single client
    /// will only intend to send one type of request and receive one type of reply. This isn't
    /// encoded as type parameters in `ClientHandle` because doing so would make it harder to
    /// run multiple different clients on the same event loop.
    #[inline]
    pub fn rpc_fut<Req, Rep>(&self, req: &Req) -> ::Result<Future<Rep>>
        where Req: serde::Serialize,
              Rep: serde::Deserialize
    {
        self.register.rpc_fut(self.token, req)
    }

    /// Send a request to the server and call the given callback when the reply is available.
    ///
    /// This method is generic over the request and the reply type, but typically a single client
    /// will only intend to send one type of request and receive one type of reply. This isn't
    /// encoded as type parameters in `ClientHandle` because doing so would make it harder to
    /// run multiple different clients on the same event loop.
    pub fn rpc<Req, Rep, F>(&self, req: &Req, rep: F) -> ::Result<()>
        where Req: serde::Serialize,
              Rep: serde::Deserialize,
              F: FnOnce(::Result<Rep>) + Send + 'static
    {
        self.register.rpc(self.token, req, rep)
    }

    /// Deregisters the client from the event loop and returns the client.
    pub fn deregister(self) -> ::Result<Client> {
        self.register.deregister(self.token)
    }

    /// Shuts down the underlying event loop.
    pub fn shutdown(self) -> ::Result<()> {
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

/// A handle to the event loop for registering and deregistering clients.
#[derive(Clone)]
pub struct Registry {
    handle: Sender<Action>,
}

impl Registry {
    /// Register a new client communicating with a service over the given socket.
    /// Returns a handle used to send commands to the client.
    pub fn register<A>(&self, addr: A) -> ::Result<ClientHandle>
        where A: ToSocketAddrs
    {
        let addr = if let Some(a) = try!(addr.to_socket_addrs()).next() {
            a
        } else {
            return Err(Error::NoAddressFound);
        };
        let socket = try!(TcpStream::connect(&addr));
        let (tx, rx) = mpsc::channel();
        try!(self.handle.send(Action::Register(socket, tx)));
        Ok(ClientHandle {
            token: try!(rx.recv()),
            register: self.clone(),
        })
    }

    /// Deregisters a client from the event loop.
    pub fn deregister(&self, token: Token) -> ::Result<Client> {
        let (tx, rx) = mpsc::channel();
        try!(self.handle.send(Action::Deregister(token, tx)));
        rx.recv().map_err(Error::from)
    }

    /// Tells the client identified by `token` to send the given request, `req`.
    #[inline]
    pub fn rpc_fut<Req, Rep>(&self, token: Token, req: &Req) -> ::Result<Future<Rep>>
        where Req: serde::Serialize,
              Rep: serde::Deserialize
    {
        let (tx, rx) = mpsc::channel();
        try!(self.handle.send(Action::Rpc(token,
                                          try!(serialize(&req)),
                                          ReplyHandler::Sender(SenderType::Mpsc(tx)))));
        Ok(Future {
            rx: rx,
            phantom_data: PhantomData,
        })
    }

    /// Tells the client identified by `token` to send the given request, calling the given
    /// callback, `rep`, when the reply is available.
    pub fn rpc<Req, Rep, F>(&self, token: Token, req: &Req, rep: F) -> ::Result<()>
        where Req: serde::Serialize,
              Rep: serde::Deserialize,
              F: FnOnce(::Result<Rep>) + Send + 'static
    {
        let callback = ReplyHandler::Callback(Box::new(move |result| match result {
            Ok(payload) => rep(deserialize(&payload)),
            Err(e) => rep(Err(e)),
        }));
        try!(self.handle.send(Action::Rpc(token, try!(serialize(&req)), callback)));
        Ok(())
    }

    /// Shuts down the underlying event loop.
    pub fn shutdown(&self) -> ::Result<()> {
        try!(self.handle.send(Action::Shutdown));
        Ok(())
    }
}

/// A thin wrapper around a `mpsc::Receiver` that handles the deserialization of server replies.
#[derive(Debug)]
pub struct Future<T: serde::Deserialize> {
    rx: mpsc::Receiver<::Result<Vec<u8>>>,
    phantom_data: PhantomData<T>,
}

impl<T: serde::Deserialize> Future<T> {
    /// Consumes the future, blocking until the server reply is available.
    pub fn get(self) -> ::Result<T> {
        deserialize(&try!(try!(self.rx.recv()))).map_err(Error::from)
    }
}

impl Handler for Dispatcher {
    type Timeout = ();
    type Message = Action;

    #[inline]
    fn ready(&mut self, event_loop: &mut EventLoop<Self>, token: Token, events: EventSet) {
        let mut client = match self.handlers.entry(token) {
            Entry::Occupied(client) => client,
            Entry::Vacant(..) => unreachable!(),
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

    #[inline]
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
                    error!("Dispatcher: failed to deregister client {:?}, {:?}",
                           token,
                           e);
                }
                if let Err(e) = tx.send(client) {
                    error!("Dispatcher: failed to send deregistered client {:?}, {:?}",
                           token,
                           e);
                }
            }
            Action::Rpc(token, payload, reply_handler) => {
                let id = self.next_rpc_id;
                self.next_rpc_id += 1;
                match self.handlers.get_mut(&token) {
                    Some(handler) => handler.on_notify(event_loop, (id, payload, reply_handler)),
                    None => reply_handler.handle(Err(Error::ConnectionBroken)),
                }
            }
            Action::Shutdown => {
                info!("Shutting down event loop.");
                for (_, mut client) in self.handlers.drain() {
                    if let Err(e) = client.deregister(event_loop) {
                        error!("Dispatcher: failed to deregister client {:?}, {:?}",
                               client.token,
                               e);
                    }
                }
                event_loop.shutdown();
            }
        }
    }
}

/// The actions that can be requested of a client. Typically users will not use `Action` directly,
/// but it is made public so that it can be examined if any errors occur when clients attempt
/// actions.
pub enum Action {
    /// Register a client on the event loop.
    Register(TcpStream, mpsc::Sender<Token>),
    /// Deregister a client running on the event loop, cancelling all in-flight rpcs.
    Deregister(Token, mpsc::Sender<Client>),
    /// Start a new rpc.
    Rpc(Token, Vec<u8>, ReplyHandler),
    /// Shut down the event loop, cancelling all in-flight rpcs on all clients.
    Shutdown,
}
