// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::SizeLimit;
use bincode::serde::{deserialize_from, serialize};
use mio::*;
use mio::tcp::TcpStream;
use protocol::Error;
use serde;
use std::collections::{HashMap, VecDeque};
use std::collections::hash_map::Entry;
use std::convert;
use std::io::{self, Cursor};
use std::marker::PhantomData;
use std::net::ToSocketAddrs;
use std::sync::mpsc;
use std::thread;
use super::{ReadState, WriteState};

struct RegisterError(NotifyError<Action>);
struct DeregisterError(NotifyError<Action>);
struct ShutdownError(NotifyError<Action>);
struct RpcError(NotifyError<Action>);

macro_rules! from_err {
    ($from:ty, $to:expr) => {
        impl convert::From<$from> for Error {
            fn from(e: $from) -> Self {
                $to(discard_inner(e.0))
            }
        }
    }
}
from_err!(RegisterError, Error::Register);
from_err!(ShutdownError, Error::Shutdown);
from_err!(RpcError, Error::Rpc);
from_err!(DeregisterError, Error::Deregister);

fn discard_inner(e: NotifyError<Action>) -> NotifyError<()> {
    match e {
        NotifyError::Io(e) => NotifyError::Io(e),
        NotifyError::Full(Action::Register(..)) => NotifyError::Full(()),
        NotifyError::Full(_) => unreachable!(),
        NotifyError::Closed(Some(Action::Register(..))) => NotifyError::Closed(None),
        NotifyError::Closed(Some(_)) => unreachable!(),
        NotifyError::Closed(None) => NotifyError::Closed(None),
    }
}

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

pub struct Packet {
    pub id: u64,
    pub payload: Vec<u8>,
}

/// The client.
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
    /// Make a new Client.
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

    pub fn dial(dialer: &::transport::tcp::TcpDialer) -> ::Result<ClientHandle>
    {
        Client::spawn(&dialer.0)
    }

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

pub struct ClientHandle
{
    token: Token,
    register: Register,
}

impl ClientHandle
{
    pub fn rpc<Req, Rep>(&self, req: &Req) -> Result<Future<Rep>, Error>
        where Req: serde::Serialize,
              Rep: serde::Deserialize,
    {
        self.register.rpc(self.token, req)
    }

    pub fn deregister(self) -> Result<Client, Error> {
        self.register.deregister(self.token)
    }

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

pub struct Dispatcher {
    handlers: HashMap<Token, Client>,
    next_handler_id: usize,
    next_rpc_id: u64,
}

impl Dispatcher {
    pub fn new() -> Dispatcher {
        Dispatcher {
            handlers: HashMap::new(),
            next_handler_id: 0,
            next_rpc_id: 0,
        }
    }

    pub fn spawn() -> Register {
        let mut event_loop = EventLoop::new().expect("D:");
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

    pub fn shutdown(&self) -> Result<(), Error> {
        self.handle.send(Action::Shutdown).map_err(|e| ShutdownError(e))?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct Future<T: serde::Deserialize> {
    rx: mpsc::Receiver<Result<Vec<u8>, Error>>,
    phantom_data: PhantomData<T>,
}

impl<T: serde::Deserialize> Future<T> {
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

pub enum Action {
    Register(TcpStream, mpsc::Sender<Token>),
    Deregister(Token, mpsc::Sender<Client>),
    Rpc(Token, Vec<u8>, SenderType),
    Shutdown,
}
