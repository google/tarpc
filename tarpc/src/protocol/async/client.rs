// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::{self, SizeLimit};
use bincode::serde::{deserialize_from, serialize, serialized_size};
use byteorder::{BigEndian, ByteOrder, ReadBytesExt};
use mio::*;
use mio::tcp::TcpStream;
use self::DeserializeState::*;
use self::SerializeState::*;
use serde;
use std::collections::{HashMap, VecDeque};
use std::fmt::Debug;
use std::io;
use std::sync::{Arc, mpsc};
use protocol::Packet;

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
pub enum DeserializeState {
    /// Tracks how many bytes of the message size have been read.
    ReadSize(u8, [u8; 8]),
    /// Tracks read progress.
    ReadData {
        /// Total length of message being read.
        message_len: usize,
        /// Length already read.
        read: usize,
        /// Buffer to read into.
        buf: Vec<u8>,
    },
}

impl DeserializeState {
    fn init() -> DeserializeState {
        ReadSize(0, [0; 8])
    }
}


pub enum SerializeState {
    WriteSize(u8, [u8; 8]),
    WriteData(Vec<u8>),
}

/** Two types of ways of receiving messages from Client. */
pub enum SenderType<Reply>
    where Reply: Send
{
    /** The nonblocking way. */
    Mio(Sender<Result<Reply, Arc<Error>>>),
    /** And the blocking way. */
    Mpsc(mpsc::Sender<Result<Reply, Arc<Error>>>),
}

/// The client.
pub struct Client<Request, Reply>
    where Request: serde::ser::Serialize + Send,
          Reply: serde::de::Deserialize + Send
{
    socket: TcpStream,
    outbound: VecDeque<(u64, Request)>,
    inbound: HashMap<u64, SenderType<Reply>>,
    tx: Option<SerializeState>,
    rx: DeserializeState,
    token: Token,
    interest: EventSet,
    next_id: u64,
}

impl<Request, Reply> Client<Request, Reply>
    where Request: serde::ser::Serialize + Send + Debug,
          Reply: serde::de::Deserialize + Send
{
    /// Make a new Client.
    pub fn new(sock: TcpStream) -> Client<Request, Reply> {
        Client {
            socket: sock,
            outbound: VecDeque::new(),
            inbound: HashMap::new(),
            tx: None,
            rx: DeserializeState::init(),
            token: Token(0),
            interest: EventSet::none(),
            next_id: 1,
        }
    }

    fn writable(&mut self, event_loop: &mut EventLoop<Self>) -> io::Result<()> {
        debug!("Client socket writable.");
        let update = match self.tx {
            None => {
                match self.outbound.get(0) {
                    Some(&(_, ref req)) => {
                        let mut buf = [0; 8];
                        let size = serialized_size(req);
                        info!("Req: {:?}, size: {}", req, size);
                        BigEndian::write_u64(&mut buf, size);
                        Some(Some(WriteSize(0, buf)))
                    }
                    None => {
                        self.interest.remove(EventSet::writable());
                        None
                    }
                }
            }
            Some(WriteSize(ref mut cur_size, ref mut buf)) => {
                match self.socket.try_write(&mut buf[*cur_size as usize..]) {
                    Ok(None) => {
                        debug!("Client: spurious wakeup while writing size.");
                        None
                    }
                    Ok(Some(bytes_written)) => {
                        debug!("Client: wrote {} bytes of size.", bytes_written);
                        *cur_size += bytes_written as u8;
                        if *cur_size == 8 {
                            let (id, message) = self.outbound.pop_front().unwrap();
                            let serialized = serialize(&Packet {
                                                           rpc_id: id,
                                                           message: message,
                                                       },
                                                       SizeLimit::Infinite)
                                                 .unwrap();
                            Some(Some(WriteData(serialized)))
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        debug!("Client: read err, {:?}", e);
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
                        debug!("Client wrote {} bytes.", written);
                        *buf = buf.split_off(written);
                        if buf.is_empty() {
                            debug!("Client finished writing; removing EventSet::writable().");
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

    fn readable(&mut self, event_loop: &mut EventLoop<Self>) -> io::Result<()> {
        debug!("Client socket readable.");
        let update = match self.rx {
            ReadSize(ref mut cur_size, ref mut buf) => {
                match self.socket.try_read(&mut buf[*cur_size as usize..]) {
                    Ok(None) => {
                        debug!("Client: spurious wakeup while reading size.");
                        None
                    }
                    Ok(Some(bytes_read)) => {
                        debug!("Client: read {} bytes of size.", bytes_read);
                        *cur_size += bytes_read as u8;
                        if *cur_size == 8 {
                            let message_len = (buf as &[u8]).read_u64::<BigEndian>().unwrap();
                            Some(ReadData {
                                message_len: message_len as usize,
                                read: 0,
                                buf: vec![0; message_len as usize],
                            })
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        debug!("Client: read err, {:?}", e);
                        None
                    }
                }
            }
            ReadData { message_len, ref mut read, ref mut buf } => {
                match self.socket.try_read(&mut buf[*read..]) {
                    Ok(None) => {
                        debug!("Client: spurious wakeup while reading data.");
                        None
                    }
                    Ok(Some(bytes_read)) => {
                        *read += bytes_read;
                        debug!("Client: read {} more bytes of data for a total of {}; {} needed",
                               bytes_read,
                               *read,
                               message_len);
                        if *read == message_len {
                            let reply: Result<Packet<Reply>, _> =
                                deserialize_from(&mut &buf[..], SizeLimit::Infinite);
                            match reply {
                                Err(e) => {
                                    let e = Arc::new(Error::from(e));
                                    for (_, tx) in self.inbound.drain() {
                                        let e = Err(e.clone());
                                        match tx {
                                            SenderType::Mpsc(tx) => {
                                                let _ = tx.send(e);
                                            }
                                            SenderType::Mio(tx) => {
                                                let _ = tx.send(e);
                                            }
                                        }
                                    }
                                    event_loop.shutdown();
                                }
                                Ok(packet) => {
                                    if let Some(tx) = self.inbound.remove(&packet.rpc_id) {
                                        match tx {
                                            SenderType::Mpsc(tx) => {
                                                if let Err(e) = tx.send(Ok(packet.message)) {
                                                    info!("Client: could not complete reply: {:?}",
                                                          e);
                                                }
                                            }
                                            SenderType::Mio(tx) => {
                                                if let Err(e) = tx.send(Ok(packet.message)) {
                                                    info!("Client: could not complete reply: {:?}",
                                                          e);
                                                }
                                            }
                                        }
                                    } else {
                                        warn!("Client: expected sender for id {} but got None!",
                                              packet.rpc_id);
                                    }
                                }
                            }
                            Some(DeserializeState::init())
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        debug!("Client: read err, {:?}", e);
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
}

impl<Request, Reply> Handler for Client<Request, Reply>
    where Request: serde::ser::Serialize + Send + Debug,
          Reply: serde::de::Deserialize + Send
{
    type Timeout = ();
    type Message = (Request, SenderType<Reply>);

    fn ready(&mut self, event_loop: &mut EventLoop<Self>, token: Token, events: EventSet) {
        debug!("Client ready: {:?} {:?}", token, events);
        assert_eq!(token, self.token);
        if events.is_readable() {
            self.readable(event_loop).unwrap();
        }
        if events.is_writable() {
            self.writable(event_loop).unwrap();
        }
    }

    fn notify(&mut self, event_loop: &mut EventLoop<Self>, (req, sender_type): Self::Message) {
        let id = self.next_id;
        self.next_id += 1;
        self.outbound.push_back((id, req));
        self.inbound.insert(id, sender_type);
        self.interest.insert(EventSet::writable());
        if let Err(e) = event_loop.reregister(&self.socket,
                                              self.token,
                                              self.interest,
                                              PollOpt::edge() | PollOpt::oneshot()) {
            warn!("Couldn't register with event loop. :'( {:?}", e);
        }
    }
}
