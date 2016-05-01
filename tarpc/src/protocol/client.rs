// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::SizeLimit;
use bincode::serde::{deserialize_from, serialize};
use byteorder::{BigEndian, ByteOrder, ReadBytesExt};
use mio::*;
use mio::tcp::TcpStream;
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

/// Public for testing.
pub enum ReadState {
    /// Tracks how many bytes of the message ID have been read.
    ReadId {
        read: u8,
        buf: [u8; 8],
    },
    /// Tracks how many bytes of the message size have been read.
    ReadSize {
        id: u64,
        read: u8,
        buf: [u8; 8],
    },
    /// Tracks read progress.
    ReadData {
        /// ID of the message being read.
        id: u64,
        /// Total length of message being read.
        message_len: usize,
        /// Length already read.
        read: usize,
        /// Buffer to read into.
        buf: Vec<u8>,
    },
}

impl ReadState {
    fn init() -> ReadState {
        ReadId {
            read: 0,
            buf: [0; 8],
        }
    }
}


pub enum WriteState {
    WriteId {
        written: u8,
        id: [u8; 8],
        size: [u8; 8],
        payload: Vec<u8>,
    },
    WriteSize {
        written: u8,
        size: [u8; 8],
        payload: Vec<u8>,
    },
    WriteData(Vec<u8>),
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

struct Packet {
    id: u64,
    payload: Vec<u8>,
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
    next_id: u64,
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
            next_id: 0,
        }
    }

    pub fn dial<Req, Rep>(dialer: &::transport::tcp::TcpDialer)
        -> ::Result<ClientHandle<Req, Rep>>
        where Req: serde::Serialize,
              Rep: serde::Deserialize
    {
        Client::spawn(&dialer.0)
    }

    pub fn spawn<Req, Rep, A>(addr: A) -> Result<ClientHandle<Req, Rep>, Error>
        where Req: serde::Serialize,
              Rep: serde::Deserialize,
              A: ToSocketAddrs,
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
        let update = match self.tx {
            None => {
                match self.outbound.pop_front() {
                    Some(packet) => {
                        let size = packet.payload.len() as u64;
                        info!("Req: id: {}, size: {}, paylod: {:?}",
                              packet.id,
                              size,
                              packet.payload);

                        let mut id_buf = [0; 8];
                        BigEndian::write_u64(&mut id_buf, packet.id);

                        let mut size_buf = [0; 8];
                        BigEndian::write_u64(&mut size_buf, size);

                        Some(Some(WriteId {
                            written: 0,
                            id: id_buf,
                            size: size_buf,
                            payload: packet.payload,
                        }))
                    }
                    None => {
                        self.interest.remove(EventSet::writable());
                        None
                    }
                }
            }
            Some(WriteId { ref mut written, mut id, size, ref mut payload }) => {
                match self.socket.try_write(&mut id[*written as usize..]) {
                    Ok(None) => {
                        debug!("Client {:?}: spurious wakeup while writing id.", self.token);
                        None
                    }
                    Ok(Some(bytes_written)) => {
                        debug!("Client {:?}: wrote {} bytes of id.",
                               self.token,
                               bytes_written);
                        *written += bytes_written as u8;
                        if *written == 8 {
                            debug!("Client {:?}: done writing id.", self.token);
                            Some(Some(WriteSize {
                                written: 0,
                                size: size,
                                payload: payload.split_off(0),
                            }))
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        debug!("Client {:?}: write err, {:?}", self.token, e);
                        self.interest.remove(EventSet::writable());
                        Some(None)
                    }
                }
            }
            Some(WriteSize { ref mut written, mut size, ref mut payload }) => {
                match self.socket.try_write(&mut size[*written as usize..]) {
                    Ok(None) => {
                        debug!("Client {:?}: spurious wakeup while writing size.",
                               self.token);
                        None
                    }
                    Ok(Some(bytes_written)) => {
                        debug!("Client {:?}: wrote {} bytes of size.",
                               self.token,
                               bytes_written);
                        *written += bytes_written as u8;
                        if *written == 8 {
                            debug!("Client {:?}: done writing size.", self.token);
                            Some(Some(WriteData(payload.split_off(0))))
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        debug!("Client {:?}: write err, {:?}", self.token, e);
                        self.interest.remove(EventSet::writable());
                        Some(None)
                    }
                }
            }
            Some(WriteData(ref mut buf)) => {
                match self.socket.try_write(buf) {
                    Ok(None) => {
                        debug!("Client flushing buf; WOULDBLOCK");
                        None
                    }
                    Ok(Some(written)) => {
                        debug!("Client wrote {} bytes of payload.", written);
                        *buf = buf.split_off(written);
                        if buf.is_empty() {
                            debug!("Client finished writing;");
                            self.interest.insert(EventSet::readable());
                            debug!("Remaining interests: {:?}", self.interest);
                            Some(None)
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        debug!("Client write error: {:?}", e);
                        self.interest.remove(EventSet::writable());
                        Some(None)
                    }
                }
            }
        };
        if let Some(tx) = update {
            self.tx = tx;
        }
        event_loop.reregister(&self.socket,
                              self.token,
                              self.interest,
                              PollOpt::edge() | PollOpt::oneshot())
    }

    fn readable<H: Handler>(&mut self, event_loop: &mut EventLoop<H>) -> io::Result<()> {
        debug!("Client {:?}: socket readable.", self.token);
        let update = match self.rx {
            ReadId { ref mut read, ref mut buf } => {
                debug!("Client {:?}: reading id.", self.token);
                match self.socket.try_read(&mut buf[*read as usize..]) {
                    Ok(None) => {
                        debug!("Client {:?}: spurious wakeup while reading id.", self.token);
                        None
                    }
                    Ok(Some(bytes_read)) => {
                        debug!("Client {:?}: read {} bytes of id.", self.token, bytes_read);
                        *read += bytes_read as u8;
                        if *read == 8 {
                            let id = (buf as &[u8]).read_u64::<BigEndian>().unwrap();
                            debug!("Client {:?}: read id {}.", self.token, id);
                            Some(ReadSize {
                                id: id,
                                read: 0,
                                buf: [0; 8],
                            })
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        debug!("Client {:?}: read err, {:?}", self.token, e);
                        self.interest.remove(EventSet::readable());
                        None
                    }
                }
            }
            ReadSize { id, ref mut read, ref mut buf } => {
                match self.socket.try_read(&mut buf[*read as usize..]) {
                    Ok(None) => {
                        debug!("Client {:?}: spurious wakeup while reading size.",
                               self.token);
                        None
                    }
                    Ok(Some(bytes_read)) => {
                        debug!("Client {:?}: read {} bytes of size.",
                               self.token,
                               bytes_read);
                        *read += bytes_read as u8;
                        if *read == 8 {
                            let message_len = (buf as &[u8]).read_u64::<BigEndian>().unwrap();
                            Some(ReadData {
                                id: id,
                                message_len: message_len as usize,
                                read: 0,
                                buf: vec![0; message_len as usize],
                            })
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        debug!("Client {:?}: read err, {:?}", self.token, e);
                        self.interest.remove(EventSet::readable());
                        None
                    }
                }
            }
            ReadData { id, message_len, ref mut read, ref mut buf } => {
                match self.socket.try_read(&mut buf[*read..]) {
                    Ok(None) => {
                        debug!("Client {:?}: spurious wakeup while reading data.",
                               self.token);
                        None
                    }
                    Ok(Some(bytes_read)) => {
                        *read += bytes_read;
                        debug!("Client {:?}: read {} more bytes of data for a total of {}; {} \
                                needed",
                               self.token,
                               bytes_read,
                               *read,
                               message_len);
                        if *read == message_len {
                            let payload = buf.split_off(0);
                            if let Some(tx) = self.inbound.remove(&id) {
                                tx.send(Ok(payload));
                            } else {
                                warn!("Client: expected sender for id {} but got None!", id);
                            }
                            Some(ReadState::init())
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        debug!("Client {:?}: read err, {:?}", self.token, e);
                        self.interest.remove(EventSet::readable());
                        None
                    }
                }
            }
        };
        if let Some(rx) = update {
            self.rx = rx;
        }
        event_loop.reregister(&self.socket,
                              self.token,
                              self.interest,
                              PollOpt::edge() | PollOpt::oneshot())
    }

    fn on_ready<H: Handler>(&mut self,
                            event_loop: &mut EventLoop<H>,
                            token: Token,
                            events: EventSet) -> bool {
        debug!("Client {:?}: ready: {:?}", token, events);
        assert_eq!(token, self.token);
        let mut action_taken = false;
        if events.is_readable() {
            self.readable(event_loop).unwrap();
            action_taken = true;
        }
        if events.is_writable() {
            self.writable(event_loop).unwrap();
            action_taken = true;
        }
        action_taken
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
        if let Err(e) = event_loop.reregister(&self.socket,
                                              self.token,
                                              self.interest,
                                              PollOpt::edge() | PollOpt::oneshot()) {
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
}

impl Handler for Client {
    type Timeout = ();
    type Message = (Vec<u8>, SenderType);

    fn ready(&mut self, event_loop: &mut EventLoop<Self>, token: Token, events: EventSet) {
        self.on_ready::<Self>(event_loop, token, events);
    }

    fn notify(&mut self, event_loop: &mut EventLoop<Self>, (req, sender_type): Self::Message) {
        let id = self.next_id;
        self.next_id += 1;
        self.on_notify(event_loop, (id, req, sender_type));
    }
}

pub struct ClientHandle<Req, Rep>
    where Req: serde::Serialize,
          Rep: serde::Deserialize,
{
    token: Token,
    register: Register,
    request: PhantomData<Req>,
    reply: PhantomData<Rep>,
}

impl<Req, Rep> ClientHandle<Req, Rep>
    where Req: serde::Serialize,
          Rep: serde::Deserialize
{
    pub fn rpc(&self, req: &Req) -> Result<Future<Rep>, Error> {
        self.register.rpc(self.token, req)
    }

    pub fn deregister(&self) -> Result<Client, Error> {
        self.register.deregister(self.token)
    }

    pub fn shutdown(self) -> Result<(), Error> {
        self.register.shutdown()
    }
}

impl<Req, Rep> Clone for ClientHandle<Req, Rep>
    where Req: serde::Serialize,
          Rep: serde::Deserialize,
{
    fn clone(&self) -> Self {
        ClientHandle {
            token: self.token,
            register: self.register.clone(),
            request: PhantomData,
            reply: PhantomData,
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
    pub fn register<Req, Rep>(&self, socket: TcpStream) -> Result<ClientHandle<Req, Rep>, Error>
        where Req: serde::Serialize,
              Rep: serde::Deserialize
    {
        let (tx, rx) = mpsc::channel();
        self.handle.send(Action::Register(socket, tx)).map_err(|e| RegisterError(e))?;
        Ok(ClientHandle {
            token: rx.recv()?,
            register: self.clone(),
            request: PhantomData,
            reply: PhantomData,
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
        self.handlers.get_mut(&token).unwrap().on_ready(event_loop, token, events);
        if events.is_hup() {
            info!("Handler {:?} socket hung up. Deregistering...", token);
            let mut client = self.handlers.remove(&token).expect(pos!());
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
                    warn!("Dispatcher: failed to register client {:?}, {:?}", token, e);
                }
                self.handlers.insert(token, client);
                if let Err(e) = tx.send(token) {
                    warn!("Dispatcher: failed to send new client's token, {:?}", e);
                }
            }
            Action::Deregister(token, tx) => {
                let mut client = self.handlers.remove(&token).unwrap();
                if let Err(e) = client.deregister(event_loop) {
                    warn!("Dispatcher: failed to deregister client {:?}, {:?}",
                          token,
                          e);
                }
                if let Err(e) = tx.send(client) {
                    warn!("Dispatcher: failed to send deregistered client {:?}, {:?}",
                          token,
                          e);
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
