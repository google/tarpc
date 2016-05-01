// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use serde;

/// The client.
pub struct Connection<Req: serde::Deserialize> {
    socket: TcpStream,
    tx: Option<WriteState>,
    rx: ReadState,
    token: Token,
    interest: EventSet,
    service: Service<Req>
}

impl Connection {
    /// Make a new Client.
    fn new(token: Token, sock: TcpStream) -> Client {
        Client {
            socket: sock,
            tx: None,
            rx: ReadState::init(),
            token: token,
            interest: EventSet::hup(),
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
