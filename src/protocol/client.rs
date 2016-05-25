// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use fnv::FnvHasher;
use mio::*;
use rand::{Rng, ThreadRng, thread_rng};
use serde;
use std::cmp;
use std::collections::{HashMap, VecDeque};
use std::collections::hash_map::Entry;
use std::convert::TryInto;
use std::fmt;
use std::hash::BuildHasherDefault;
use std::io;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::thread;
use super::{ReadState, RpcId, Stream, deserialize, serialize};
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
    /// Create a new client that connects to the given stream.
    ///
    /// Valid arguments include `Stream`, as well as `String` or anything else that
    /// converts into a `SocketAddr`.
    fn connect<S>(stream: S) -> ::Result<Self>
        where S: TryInto<Stream, Err=Error>
    {
        Self::register(stream, &*REGISTRY)
    }

    /// Register a new client that connects to the given stream.
    ///
    /// Valid arguments include `Stream`, as well as `String` or anything else that
    /// converts into a `SocketAddr`.
    fn register<S>(stream: S, registry: &Registry) -> ::Result<Self>
        where S: TryInto<Stream, Err=Error>;
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

/// Clients initially retry after one second.
const DEFAULT_TIMEOUT_MS: u64 = 1_000;

/// Clients cap the time between retries at 30 seconds.
const MAX_TIMEOUT_MS: u64 = 30_000;

/// Given the number of attempts thus far, calculates the time to wait before sending the next
/// request.
fn backoff_with_jitter<R: Rng>(attempt: u32, rng: &mut R) -> u64 {
    let max = cmp::min(MAX_TIMEOUT_MS,
                       DEFAULT_TIMEOUT_MS.saturating_mul(backoff_factor(attempt)));
    rng.gen_range(0, max)
}

fn backoff_factor(attempt: u32) -> u64 {
    use std::u64;
    1u64.checked_shl(attempt).unwrap_or(u64::MAX)
}

/// A low-level client for communicating with a service. Reads and writes byte buffers. Typically
/// a type-aware client will be built on top of this.
pub struct AsyncClient {
    socket: Stream,
    next_rpc_id: RpcId,
    outbound: VecDeque<Packet>,
    inbound: HashMap<RpcId, RequestContext, BuildHasherDefault<FnvHasher>>,
    tx: Option<WriteState>,
    rx: ReadState,
    token: Token,
    interest: EventSet,
    timeout: Option<Timeout>,
    /// How many times have we tried to send since the last successful request?
    request_attempt: u32,
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
    fn new(token: Token, sock: Stream) -> AsyncClient {
        AsyncClient {
            socket: sock,
            next_rpc_id: RpcId(0),
            outbound: VecDeque::new(),
            inbound: HashMap::with_hasher(BuildHasherDefault::default()),
            tx: None,
            rx: ReadState::init(),
            token: token,
            interest: EventSet::readable() | EventSet::hup(),
            timeout: None,
            // Default timeout is 1 second.
            request_attempt: 0,
        }
    }

    /// Starts an event loop on a thread and registers a new client connected to the given address.
    pub fn connect<S>(stream: S) -> ::Result<ClientHandle>
        where S: TryInto<Stream, Err=Error>
    {
        REGISTRY.clone().connect(stream)
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

    fn readable<R: Rng>(&mut self,
                        rng: &mut R,
                        event_loop: &mut EventLoop<Dispatcher>)
                        -> io::Result<()> {
        debug!("AsyncClient {:?}: socket readable.", self.token);
        {
            let inbound = &mut self.inbound;
            if let Some(packet) = ReadState::next(&mut self.rx, &mut self.socket, self.token) {
                if let Entry::Occupied(mut entry) = inbound.entry(packet.id) {
                    match entry.get_mut().callback.take().unwrap().handle(Ok(packet.payload)) {
                        RequestAction::Retry(cb) => {
                            entry.get_mut().callback = Some(cb);
                            // If we're not writable, but there are pending rpc's, it's because
                            // there's already another timeout set. No reason to set two at once.
                            if self.outbound.is_empty() || self.interest.is_writable() {
                                let retry_in = backoff_with_jitter(self.request_attempt, rng);
                                debug!("AsyncClient {:?}: request {:?} returned Busy. Resuming \
                                       requests in {}ms",
                                       self.token,
                                       packet.id,
                                       retry_in);
                                match event_loop.timeout_ms(self.token, retry_in) {
                                    Ok(timeout) => {
                                        self.timeout = Some(timeout);
                                        self.request_attempt += 1;
                                        self.interest.remove(EventSet::writable());
                                    }
                                    Err(e) => {
                                        warn!("Client {:?}: failed to set timeout, {:?}",
                                              self.token,
                                              e);
                                    }
                                }
                            }
                            self.outbound.push_back(entry.get().packet.clone());
                        }
                        RequestAction::Complete => {
                            self.request_attempt = 0;
                            entry.remove();
                        }
                    }
                } else {
                    warn!("AsyncClient: expected sender for id {:?} but got None!",
                          packet.id);
                }
            }
        }
        self.reregister(event_loop)
    }

    fn on_ready<R: Rng>(&mut self,
                        event_loop: &mut EventLoop<Dispatcher>,
                        token: Token,
                        events: EventSet,
                        rng: &mut R)
                        -> io::Result<()> {
        debug!("AsyncClient {:?}: ready: {:?}", token, events);
        assert_eq!(token, self.token);
        if events.is_readable() {
            try!(self.readable(rng, event_loop));
        }
        if events.is_writable() {
            try!(self.writable(event_loop));
        }
        Ok(())
    }

    fn rpc(&mut self,
           event_loop: &mut EventLoop<Dispatcher>,
           req: Vec<u8>,
           handler: Box<ReplyCallback + Send>) {
        let id = self.next_rpc_id;
        self.next_rpc_id += 1;
        let packet = Packet {
            id: id,
            payload: Rc::new(req),
        };
        // If no other rpc's are being written, then it wasn't writable,
        // so we need to make it writable.
        if self.outbound.is_empty() {
            self.interest.insert(EventSet::writable());
        }
        self.outbound.push_back(packet.clone());
        self.inbound.insert(id,
                            RequestContext {
                                callback: Some(handler),
                                packet: packet,
                            });
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
pub struct Dispatcher {
    clients: HashMap<Token, AsyncClient, BuildHasherDefault<FnvHasher>>,
    next_handler_id: usize,
    rng: ThreadRng,
}

impl Dispatcher {
    fn new() -> Dispatcher {
        Dispatcher {
            clients: HashMap::with_hasher(BuildHasherDefault::default()),
            next_handler_id: 0,
            rng: thread_rng(),
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
    /// Connects a new client to the service specified by the given address.
    /// Returns a handle used to send commands to the client.
    pub fn connect<S>(&self, stream: S) -> ::Result<ClientHandle>
        where S: TryInto<Stream, Err=Error>,
    {
        self.register(try!(stream.try_into()))
    }

    /// Register a new client communicating over the given stream.
    /// Returns a handle used to send commands to the client.
    pub fn register<S>(&self, stream: S) -> ::Result<ClientHandle>
        where S: TryInto<Stream, Err=Error>,
    {
        let (tx, rx) = mpsc::channel();
        try!(self.handle.send(Action::Register(try!(stream.try_into()), tx)));
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
              self.clients.keys().collect::<Vec<_>>());
        let mut client = match self.clients.entry(token) {
            Entry::Occupied(client) => client,
            Entry::Vacant(..) => unreachable!(),
        };
        if let Err(e) = client.get_mut().on_ready(event_loop, token, events, &mut self.rng) {
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
              self.clients.keys().collect::<Vec<_>>());
        match action {
            Action::Register(socket, tx) => {
                let token = Token(self.next_handler_id);
                info!("Dispatcher: registering {:?}", token);
                self.next_handler_id += 1;
                let client = AsyncClient::new(token, socket);
                if let Err(e) = client.register(event_loop) {
                    error!("Dispatcher: failed to register client {:?}, {:?}", token, e);
                }
                self.clients.insert(token, client);
                if let Err(e) = tx.send(token) {
                    error!("Dispatcher: failed to send new client's token, {:?}", e);
                }
            }
            Action::Deregister(token) => {
                info!("Dispatcher: deregistering {:?}", token);
                // If it's not present, it must have already been deregistered.
                if let Some(mut client) = self.clients.remove(&token) {
                    if let Err(e) = client.deregister(event_loop) {
                        error!("Dispatcher: failed to deregister client {:?}, {:?}",
                               token,
                               e);
                    }
                }
            }
            Action::Rpc(token, payload, callback) => {
                match self.clients.get_mut(&token) {
                    Some(handler) => handler.rpc(event_loop, payload, callback),
                    None => {
                        // Ignore return type because this is unretryable.
                        callback.handle(Err(Error::ConnectionBroken));
                    }
                }
            }
            Action::Shutdown => {
                info!("Shutting down event loop.");
                for (_, mut client) in self.clients.drain() {
                    if let Err(e) = client.deregister(event_loop) {
                        error!("Dispatcher: failed to deregister client {:?}, {:?}",
                               client.token,
                               e);
                    }
                }
                event_loop.shutdown();
            }
            Action::Debug(tx) => {
                if let Err(e) = tx.send(DebugInfo { clients: self.clients.len() }) {
                    warn!("Dispatcher: failed to send debug info, {:?}", e);
                }
            }
        }
    }

    fn timeout(&mut self, event_loop: &mut EventLoop<Self>, token: Token) {
        info!("Dispatcher: timeout {:?}", token);
        let client = self.clients.get_mut(&token).unwrap();
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
    Register(Stream, mpsc::Sender<Token>),
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

/// Information about a running client Dispatcher.
#[derive(Debug)]
pub struct DebugInfo {
    /// Number of handlers managed by the dispatcher.
    pub clients: usize,
}

#[cfg(test)]
mod tests {
    #[test]
    fn backoff_factor() {
        use std::u64;
        assert_eq!(super::backoff_factor(0), 1);
        assert_eq!(super::backoff_factor(1), 2);
        assert_eq!(super::backoff_factor(2), 4);
        assert_eq!(super::backoff_factor(3), 8);
        assert_eq!(super::backoff_factor(4), 16);
        assert_eq!(super::backoff_factor(64), u64::MAX);
        assert_eq!(super::backoff_factor(100), u64::MAX);
    }
}
