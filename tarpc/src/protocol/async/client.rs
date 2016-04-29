// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode;
use byteorder::{BigEndian, ByteOrder, ReadBytesExt};
use mio::*;
use mio::tcp::TcpStream;
use self::ReadState::*;
use self::WriteState::*;
use self::Action::*;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::{Arc, mpsc};

quick_error! {
    /// Async errors.
    #[derive(Debug)]
    pub enum Error {
        /// IO error.
        Io(err: io::Error) {
            from()
            description(err.description())
        }
        /// Serialization error.
        Bincode(err: bincode::serde::DeserializeError) {
            from()
            description(err.description())
        }
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
    Mio(Sender<Result<Vec<u8>, Arc<Error>>>),
    /** And the blocking way. */
    Mpsc(mpsc::Sender<Result<Vec<u8>, Arc<Error>>>),
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
    pub fn new(sock: TcpStream) -> Client {
        Client {
            socket: sock,
            outbound: VecDeque::new(),
            inbound: HashMap::new(),
            tx: None,
            rx: ReadState::init(),
            token: Token(0),
            interest: EventSet::none(),
            next_id: 0,
        }
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
                        debug!("Client {:?}: read err, {:?}", self.token, e);
                        None
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
                        debug!("Client {:?}: read err, {:?}", self.token, e);
                        None
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
                        None
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
                                match tx {
                                    SenderType::Mpsc(tx) => {
                                        if let Err(e) = tx.send(Ok(payload)) {
                                            info!("Client: could not complete reply: {:?}", e);
                                        }
                                    }
                                    SenderType::Mio(tx) => {
                                        if let Err(e) = tx.send(Ok(payload)) {
                                            info!("Client: could not complete reply: {:?}", e);
                                        }
                                    }
                                }
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
                            events: EventSet) {
        debug!("Client {:?}: ready: {:?}", token, events);
        assert_eq!(token, self.token);
        if events.is_readable() {
            self.readable(event_loop).unwrap();
        }
        if events.is_writable() {
            self.writable(event_loop).unwrap();
        }
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

    fn register<H: Handler>(&mut self, event_loop: &mut EventLoop<H>) -> io::Result<()> {
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
}

impl Handler for Dispatcher {
    type Timeout = ();
    type Message = Action;

    fn ready(&mut self, event_loop: &mut EventLoop<Self>, token: Token, events: EventSet) {
        self.handlers.get_mut(&token).unwrap().on_ready(event_loop, token, events);
    }

    fn notify(&mut self, event_loop: &mut EventLoop<Self>, action: Action) {
        match action {
            Register(mut client, tx) => {
                let token = Token(self.next_handler_id);
                self.next_handler_id += 1;
                client.token = token;
                if let Err(e) = client.register(event_loop) {
                    warn!("Dispatcher: failed to register client {:?}, {:?}", token, e);
                }
                self.handlers.insert(token, client);
                if let Err(e) = tx.send(token) {
                    warn!("Dispatcher: failed to send new client's token, {:?}", e);
                }
            }
            Deregister(token, tx) => {
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
            Rpc(token, payload, sender) => {
                let id = self.next_rpc_id;
                self.next_rpc_id += 1;
                self.handlers.get_mut(&token).unwrap().on_notify(event_loop, (id, payload, sender));
            }
            Shutdown => {
                info!("Shutting down event loop.");
                event_loop.shutdown();
            }
        }
    }
}

pub enum Action {
    Register(Client, mpsc::Sender<Token>),
    Deregister(Token, mpsc::Sender<Client>),
    Rpc(Token, Vec<u8>, SenderType),
    Shutdown,
}
