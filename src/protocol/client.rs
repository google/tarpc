// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use mio::*;
use mio::tcp::TcpStream;
use serde;
use std::collections::{HashMap, VecDeque};
use std::collections::hash_map::Entry;
use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::net::ToSocketAddrs;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::thread;
use super::{DebugInfo, ReadState, deserialize, serialize};
use {Error, RpcResult};

lazy_static! {
    /// The client global event loop on which all clients are registered by default.
    pub static ref REGISTRY: Registry = {
        let mut config = EventLoopConfig::default();
        config.notify_capacity(1_000_000);
        let mut event_loop = EventLoop::configured(config)
            .expect("Tarpc startup: could not configure the client global event loop!");
        let handle = event_loop.channel();
        thread::spawn(move || {
            if let Err(e) = event_loop.run(&mut Dispatcher::new()) {
                error!("Event loop failed: {:?}", e);
            }
        });
        Registry { handle: handle }
    };
}

type Packet = super::Packet<Rc<Vec<u8>>>;
type WriteState = super::WriteState<Rc<Vec<u8>>>;

/// A client is a stub that can connect to a service and register itself
/// on the client event loop.
pub trait Client: Sized {
    /// Create a new client that communicates over the given socket.
    fn connect<A>(addr: A) -> ::Result<Self> where A: ToSocketAddrs;

    /// Register a new client that communicates over the given socket.
    fn register<A>(addr: A, register: &Registry) -> ::Result<Self> where A: ToSocketAddrs;
}

/// A function called when the rpc reply is available.
pub trait ReplyCallback {
    /// Consumes the rpc result.
    fn handle(self: Box<Self>, result: ::Result<Vec<u8>>) -> RequestAction;
}

/// The action to take after handling an rpc reply.
pub enum RequestAction {
    /// Finish the rpc. No further action is taken.
    Complete,
    /// Resend the rpc to the server.
    Retry(Box<ReplyCallback + Send>),
}

impl<F> ReplyCallback for F
    where F: FnOnce(::Result<Vec<u8>>) -> RequestAction
{
    fn handle(self: Box<Self>, result: ::Result<Vec<u8>>) -> RequestAction {
        self(result)
    }
}

/// Things related to a request that the client needs.
struct RequestContext {
    packet: Packet,
    callback: Option<Box<ReplyCallback + Send>>,
}

impl fmt::Debug for RequestContext {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f,
               "RequestContext {{ packet: {:?}, callback: Box<ReplyCallback + Send> }}",
               self.packet)
    }
}

/// A low-level client for communicating with a service. Reads and writes byte buffers. Typically
/// a type-aware client will be built on top of this.
pub struct AsyncClient {
    socket: TcpStream,
    outbound: VecDeque<Packet>,
    inbound: HashMap<u64, RequestContext>,
    tx: Option<WriteState>,
    rx: ReadState,
    token: Token,
    interest: EventSet,
    timeout: Option<Timeout>,
}

impl fmt::Debug for AsyncClient {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f,
               "AsyncClient {{ socket: {:?}, outbound: {:?}, inbound: {:?}, tx: {:?}, \
               rx: {:?}, token: {:?}, interest: {:?}, timeout: Timeout }}",
               self.socket,
               self.outbound,
               self.inbound,
               self.tx,
               self.rx,
               self.token,
               self.interest)
    }
}
impl AsyncClient {
    fn new(token: Token, sock: TcpStream) -> AsyncClient {
        AsyncClient {
            socket: sock,
            outbound: VecDeque::new(),
            inbound: HashMap::new(),
            tx: None,
            rx: ReadState::init(),
            token: token,
            interest: EventSet::readable() | EventSet::hup(),
            timeout: None,
        }
    }

    /// Starts an event loop on a thread and registers a new client connected to the given address.
    pub fn connect<A>(addr: A) -> ::Result<ClientHandle>
        where A: ToSocketAddrs
    {
        REGISTRY.clone().register(addr)
    }

    fn writable(&mut self, event_loop: &mut EventLoop<Dispatcher>) -> io::Result<()> {
        debug!("AsyncClient socket writable.");
        WriteState::next(&mut self.tx,
                         &mut self.socket,
                         &mut self.outbound,
                         &mut self.interest,
                         self.token);
        self.reregister(event_loop)
    }

    fn readable(&mut self, event_loop: &mut EventLoop<Dispatcher>) -> io::Result<()> {
        debug!("AsyncClient {:?}: socket readable.", self.token);
        {
            let inbound = &mut self.inbound;
            if let Some(packet) = ReadState::next(&mut self.rx, &mut self.socket, self.token) {
                if let Entry::Occupied(mut entry) = inbound.entry(packet.id) {
                    match entry.get_mut().callback.take().unwrap().handle(Ok(packet.payload)) {
                        RequestAction::Retry(cb) => {
                            entry.get_mut().callback = Some(cb);
                            info!("Retrying request {:?}", packet.id);
                            self.outbound.push_back(entry.get().packet.clone());
                            match event_loop.timeout_ms(self.token, 1000) {
                                Ok(_) => self.interest.remove(EventSet::writable()),
                                Err(e) => {
                                    warn!("Client {:?}: failed to set timeout, {:?}",
                                          self.token,
                                          e);
                                }
                            }
                        }
                        RequestAction::Complete => {
                            entry.remove();
                        }
                    }
                } else {
                    warn!("AsyncClient: expected sender for id {} but got None!",
                          packet.id);
                }
            }
        }
        self.reregister(event_loop)
    }

    fn on_ready(&mut self,
                event_loop: &mut EventLoop<Dispatcher>,
                token: Token,
                events: EventSet)
                -> io::Result<()> {
        debug!("AsyncClient {:?}: ready: {:?}", token, events);
        assert_eq!(token, self.token);
        if events.is_readable() {
            try!(self.readable(event_loop));
        }
        if events.is_writable() {
            try!(self.writable(event_loop));
        }
        Ok(())
    }

    fn on_notify(&mut self,
                 event_loop: &mut EventLoop<Dispatcher>,
                 (id, req, handler): (u64, Vec<u8>, Box<ReplyCallback + Send>)) {
        let packet = Packet {
            id: id,
            payload: Rc::new(req),
        };
        self.outbound.push_back(packet.clone());
        self.inbound.insert(id,
                            RequestContext {
                                callback: Some(handler),
                                packet: packet,
                            });
        self.interest.insert(EventSet::writable());
        if let Err(e) = self.reregister(event_loop) {
            warn!("AsyncClient {:?}: couldn't register with event loop, {:?}",
                  self.token,
                  e);
        }
    }

    fn register(&self, event_loop: &mut EventLoop<Dispatcher>) -> io::Result<()> {
        event_loop.register(&self.socket,
                            self.token,
                            self.interest,
                            PollOpt::edge() | PollOpt::oneshot())
    }

    fn deregister(&mut self, event_loop: &mut EventLoop<Dispatcher>) -> io::Result<()> {
        for (_, sender) in self.inbound.drain() {
            sender.callback.unwrap().handle(Err(Error::ConnectionBroken));
        }
        if let Some(timeout) = self.timeout.take() {
            event_loop.clear_timeout(timeout);
        }
        event_loop.deregister(&self.socket)
    }

    fn reregister(&mut self, event_loop: &mut EventLoop<Dispatcher>) -> io::Result<()> {
        event_loop.reregister(&self.socket,
                              self.token,
                              self.interest,
                              PollOpt::edge() | PollOpt::oneshot())
    }
}

/// A thin wrapper around `Registry` that ensures messages are sent to the correct client.
#[derive(Clone, Debug)]
pub struct ClientHandle {
    token: Token,
    registry: Registry,
    count: Option<Arc<()>>,
}

impl Drop for ClientHandle {
    fn drop(&mut self) {
        info!("ClientHandle {:?}: deregistering.", self.token);
        match Arc::try_unwrap(self.count.take().unwrap()) {
            Ok(_) => {
                if let Err(e) = self.registry.deregister(self.token) {
                    warn!("ClientHandle {:?}: could not deregister, {:?}",
                          self.token,
                          e);
                }
            }
            Err(count) => self.count = Some(count),
        }
    }
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
    pub fn rpc_sync<Req, Rep>(&self, req: &Req) -> ::Result<Rep>
        where Req: serde::Serialize,
              Rep: serde::Deserialize + Send + 'static
    {
        let (tx, rx) = mpsc::channel();
        try!(self.registry.rpc(self.token, req, move |result| tx.send(result).unwrap()));
        rx.recv().unwrap()
    }

    /// Send a request to the server and call the given callback when the reply is available.
    ///
    /// This method is generic over the request and the reply type, but typically a single client
    /// will only intend to send one type of request and receive one type of reply. This isn't
    /// encoded as type parameters in `ClientHandle` because doing so would make it harder to
    /// run multiple different clients on the same event loop.
    #[inline]
    pub fn rpc<Req, Rep, F>(&self, req: &Req, rep: F) -> ::Result<()>
        where Req: serde::Serialize,
              Rep: serde::Deserialize + Send + 'static,
              F: FnOnce(::Result<Rep>) + Send + 'static
    {
        self.registry.rpc(self.token, req, rep)
    }
}

/// An event loop handler that manages multiple clients.
#[derive(Debug)]
pub struct Dispatcher {
    handlers: HashMap<Token, AsyncClient>,
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
#[derive(Clone, Debug)]
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
            registry: self.clone(),
            count: Some(Arc::new(())),
        })
    }

    /// Deregisters a client from the event loop.
    pub fn deregister(&self, token: Token) -> ::Result<()> {
        try!(self.handle.send(Action::Deregister(token)));
        Ok(())
    }

    /// Tells the client identified by `token` to send the given request, calling the given
    /// callback, `rep`, when the reply is available.
    pub fn rpc<Req, Rep, F>(&self, token: Token, req: &Req, rep: F) -> ::Result<()>
        where Req: serde::Serialize,
              Rep: serde::Deserialize + Send + 'static,
              F: FnOnce(::Result<Rep>) + Send + 'static
    {
        try!(self.handle.send(Action::Rpc(token,
                                          try!(serialize(&req)),
                                          Box::new(Callback {
                                              f: rep,
                                              phantom_data: PhantomData,
                                          }))));
        Ok(())
    }

    /// Shuts down the underlying event loop.
    pub fn shutdown(&self) -> ::Result<()> {
        try!(self.handle.send(Action::Shutdown));
        Ok(())
    }

    /// Returns debug info about the running client Dispatcher.
    pub fn debug(&self) -> ::Result<DebugInfo> {
        let (tx, rx) = mpsc::channel();
        try!(self.handle.send(Action::Debug(tx)));
        Ok(try!(rx.recv()))
    }
}

struct Callback<F, Rep> {
    f: F,
    phantom_data: PhantomData<Rep>,
}

impl<F, Rep> ReplyCallback for Callback<F, Rep>
    where F: FnOnce(::Result<Rep>) + Send + 'static,
          Rep: serde::Deserialize + Send + 'static
{
    fn handle(self: Box<Self>, result: ::Result<Vec<u8>>) -> RequestAction {
        match result {
            Ok(payload) => {
                let result = deserialize::<RpcResult<Rep>>(&payload);
                let result = result.and_then(|r| r.map_err(|e| e.into()));
                match result {
                    Err(Error::Busy) => RequestAction::Retry(self),
                    result => {
                        (self.f)(result);
                        RequestAction::Complete
                    }
                }
            }
            Err(e) => {
                (self.f)(Err(e));
                RequestAction::Complete
            }
        }
    }
}

/// An asynchronous RPC call.
#[derive(Debug)]
pub struct Future<T> {
    rx: mpsc::Receiver<::Result<T>>,
}

impl<T> Future<T>
    where T: serde::Deserialize
{
    /// Create a new `Future` by wrapping a `mpsc::Receiver`.
    pub fn new(rx: mpsc::Receiver<::Result<T>>) -> Future<T> {
        Future { rx: rx }
    }

    /// Block until the result of the RPC call is available
    pub fn get(self) -> ::Result<T> {
        try!(self.rx.recv())
    }
}

impl Handler for Dispatcher {
    type Timeout = Token;
    type Message = Action;

    #[inline]
    fn ready(&mut self, event_loop: &mut EventLoop<Self>, token: Token, events: EventSet) {
        info!("Dispatcher: clients: {:?}",
              self.handlers.keys().collect::<Vec<_>>());
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
        info!("Dispatcher: clients: {:?}",
              self.handlers.keys().collect::<Vec<_>>());
        match action {
            Action::Register(socket, tx) => {
                let token = Token(self.next_handler_id);
                info!("Dispatcher: registering {:?}", token);
                self.next_handler_id += 1;
                let client = AsyncClient::new(token, socket);
                if let Err(e) = client.register(event_loop) {
                    error!("Dispatcher: failed to register client {:?}, {:?}", token, e);
                }
                self.handlers.insert(token, client);
                if let Err(e) = tx.send(token) {
                    error!("Dispatcher: failed to send new client's token, {:?}", e);
                }
            }
            Action::Deregister(token) => {
                info!("Dispatcher: deregistering {:?}", token);
                // If it's not present, it must have already been deregistered.
                if let Some(mut client) = self.handlers.remove(&token) {
                    if let Err(e) = client.deregister(event_loop) {
                        error!("Dispatcher: failed to deregister client {:?}, {:?}",
                               token,
                               e);
                    }
                }
            }
            Action::Rpc(token, payload, callback) => {
                let id = self.next_rpc_id;
                self.next_rpc_id += 1;
                match self.handlers.get_mut(&token) {
                    Some(handler) => handler.on_notify(event_loop, (id, payload, callback)),
                    None => {
                        callback.handle(Err(Error::ConnectionBroken));
                    }
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
            Action::Debug(tx) => {
                if let Err(e) = tx.send(DebugInfo { handlers: self.handlers.len() }) {
                    warn!("Dispatcher: failed to send debug info, {:?}", e);
                }
            }
        }
    }

    fn timeout(&mut self, event_loop: &mut EventLoop<Self>, token: Token) {
        info!("Dispatcher: timeout {:?}", token);
        let client = self.handlers.get_mut(&token).unwrap();
        client.interest.insert(EventSet::writable());
        if let Err(e) = client.reregister(event_loop) {
            warn!("Dispatcher: failed to reregister client {:?}, {:?}",
                  token,
                  e);
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
    Deregister(Token),
    /// Start a new rpc.
    Rpc(Token, Vec<u8>, Box<ReplyCallback + Send>),
    /// Shut down the event loop, cancelling all in-flight rpcs on all clients.
    Shutdown,
    /// Get debug info.
    Debug(mpsc::Sender<DebugInfo>),
}

impl fmt::Debug for Action {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match *self {
            Action::Register(ref stream, ref sender) => {
                write!(f, "Action::Register({:?}, {:?})", stream, sender)
            }
            Action::Deregister(token) => write!(f, "Action::Deregister({:?})", token),
            Action::Rpc(token, ref buf, _) => {
                write!(f, "Action::Rpc({:?}, {:?}, <callback>)", token, buf)
            }
            Action::Shutdown => write!(f, "Action::Shutdown"),
            Action::Debug(ref sender) => write!(f, "Action::Debug({:?})", sender),
        }
    }
}
