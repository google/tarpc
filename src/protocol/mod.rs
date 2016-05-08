// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::SizeLimit;
use bincode::serde as bincode;
use byteorder::{BigEndian, ByteOrder, ReadBytesExt};
use mio::*;
use mio::tcp::TcpStream;
use serde;
use self::ReadState::*;
use self::WriteState::*;
use std::collections::VecDeque;
use std::io::Cursor;

/// Client-side implementation of the tarpc protocol.
pub mod client;
/// Server-side implementation of the tarpc protocol.
pub mod server;

pub use self::client::{Client, ClientHandle, Future, SenderType};
pub use self::server::{Server, Service, ServeHandle};

/// The means of communication between client and server.
pub struct Packet {
    /// Identifies the request. The reply packet should specify the same id as the request.
    pub id: u64,
    /// The payload is typically a message that the client and server deserializes
    /// before handling.
    pub payload: Vec<u8>,
}

/// A state machine that writes packets in non-blocking fashion.
enum WriteState {
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

impl WriteState {
    fn next(state: &mut Option<WriteState>,
            socket: &mut TcpStream,
            outbound: &mut VecDeque<Packet>,
            interest: &mut EventSet,
            token: Token) {
        let update = match *state {
            None => {
                match outbound.pop_front() {
                    Some(packet) => {
                        let size = packet.payload.len() as u64;
                        debug!("WriteState {:?}: Packet: id: {}, size: {}", token, packet.id, size);

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
                        interest.remove(EventSet::writable());
                        None
                    }
                }
            }
            Some(WriteId { ref mut written, mut id, size, ref mut payload }) => {
                match socket.try_write(&mut id[*written as usize..]) {
                    Ok(None) => {
                        debug!("WriteState {:?}: spurious wakeup while writing id.", token);
                        None
                    }
                    Ok(Some(bytes_written)) => {
                        debug!("WriteState {:?}: wrote {} bytes of id.", token, bytes_written);
                        *written += bytes_written as u8;
                        if *written == 8 {
                            debug!("WriteState {:?}: done writing id.", token);
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
                        debug!("WriteState {:?}: write err, {:?}", token, e);
                        interest.remove(EventSet::writable());
                        Some(None)
                    }
                }
            }
            Some(WriteSize { ref mut written, mut size, ref mut payload }) => {
                match socket.try_write(&mut size[*written as usize..]) {
                    Ok(None) => {
                        debug!("WriteState {:?}: spurious wakeup while writing size.", token);
                        None
                    }
                    Ok(Some(bytes_written)) => {
                        debug!("WriteState {:?}: wrote {} bytes of size.", token, bytes_written);
                        *written += bytes_written as u8;
                        if *written == 8 {
                            debug!("WriteState {:?}: done writing size.", token);
                            Some(Some(WriteData(payload.split_off(0))))
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        debug!("WriteState {:?}: write err, {:?}", token, e);
                        interest.remove(EventSet::writable());
                        Some(None)
                    }
                }
            }
            Some(WriteData(ref mut buf)) => {
                match socket.try_write(buf) {
                    Ok(None) => {
                        debug!("WriteState {:?}: flushing buf; WOULDBLOCK", token);
                        None
                    }
                    Ok(Some(written)) => {
                        debug!("WriteState {:?}: wrote {} bytes of payload.", token, written);
                        *buf = buf.split_off(written);
                        if buf.is_empty() {
                            debug!("WriteState {:?}: finished writing;", token);
                            interest.insert(EventSet::readable());
                            debug!("Remaining interests: {:?}", interest);
                            Some(None)
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        debug!("WriteState {:?}: write error: {:?}", token, e);
                        interest.remove(EventSet::writable());
                        Some(None)
                    }
                }
            }
        };
        if let Some(next) = update {
            *state = next;
        }
    }
}

/// A state machine that reads packets in non-blocking fashion.
enum ReadState {
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

    fn next(state: &mut ReadState,
               socket: &mut TcpStream,
               interest: &mut EventSet,
               token: Token) -> Option<Packet>
    {
        let (next, packet) = match *state {
            ReadId { ref mut read, ref mut buf } => {
                debug!("ReadState {:?}: reading id.", token);
                match socket.try_read(&mut buf[*read as usize..]) {
                    Ok(None) => {
                        debug!("ReadState {:?}: spurious wakeup while reading id.", token);
                        (None, None)
                    }
                    Ok(Some(bytes_read)) => {
                        debug!("ReadState {:?}: read {} bytes of id.", token, bytes_read);
                        *read += bytes_read as u8;
                        if *read == 8 {
                            let id = (buf as &[u8]).read_u64::<BigEndian>().unwrap();
                            debug!("ReadState {:?}: read id {}.", token, id);
                            (Some(ReadSize {
                                id: id,
                                read: 0,
                                buf: [0; 8],
                            }), None)
                        } else {
                            (None, None)
                        }
                    }
                    Err(e) => {
                        debug!("ReadState {:?}: read err, {:?}", token, e);
                        interest.remove(EventSet::readable());
                        (None, None)
                    }
                }
            }
            ReadSize { id, ref mut read, ref mut buf } => {
                match socket.try_read(&mut buf[*read as usize..]) {
                    Ok(None) => {
                        debug!("ReadState {:?}: spurious wakeup while reading size.", token);
                        (None, None)
                    }
                    Ok(Some(bytes_read)) => {
                        debug!("ReadState {:?}: read {} bytes of size.", token, bytes_read);
                        *read += bytes_read as u8;
                        if *read == 8 {
                            let message_len = (buf as &[u8]).read_u64::<BigEndian>().unwrap();
                            debug!("ReadState {:?}: message len = {}", token, message_len);
                            if message_len == 0 {
                                (Some(ReadState::init()), Some(Packet { id: id, payload: vec![] }))
                            } else {
                                (Some(ReadData {
                                    id: id,
                                    message_len: message_len as usize,
                                    read: 0,
                                    buf: vec![0; message_len as usize],
                                }), None)
                            }
                        } else {
                            (None, None)
                        }
                    }
                    Err(e) => {
                        debug!("ReadState {:?}: read err, {:?}", token, e);
                        interest.remove(EventSet::readable());
                        (None, None)
                    }
                }
            }
            ReadData { id, message_len, ref mut read, ref mut buf } => {
                match socket.try_read(&mut buf[*read..]) {
                    Ok(None) => {
                        debug!("ReadState {:?}: spurious wakeup while reading data.", token);
                        (None, None)
                    }
                    Ok(Some(bytes_read)) => {
                        *read += bytes_read;
                        debug!("ReadState {:?}: read {} more bytes of data for a total of {}; {} \
                                needed",
                               token,
                               bytes_read,
                               *read,
                               message_len);
                        if *read == message_len {
                            let payload = buf.split_off(0);
                            (Some(ReadState::init()), Some(Packet { id: id, payload: payload }))
                        } else {
                            (None, None)
                        }
                    }
                    Err(e) => {
                        debug!("ReadState {:?}: read err, {:?}", token, e);
                        interest.remove(EventSet::readable());
                        (None, None)
                    }
                }
            }
        };
        if let Some(next) = next {
            *state = next;
        }
        packet
    }
}

/// Serialize `s`. Returns `Vec<u8>` if successful, otherwise `tarpc::Error`.
pub fn serialize<S: serde::Serialize>(s: &S) -> ::Result<Vec<u8>> {
    bincode::serialize(s, SizeLimit::Infinite).map_err(|e| e.into())
}

/// Deserialize a buffer into a `D`. On error, returns `tarpc::Error`.
pub fn deserialize<D: serde::Deserialize>(buf: &Vec<u8>) -> ::Result<D> {
    bincode::deserialize_from(&mut Cursor::new(buf), SizeLimit::Infinite).map_err(|e| e.into())
}

#[cfg(test)]
mod test {
    extern crate env_logger;
    use mio::EventLoop;
    use super::{Client, Packet, Service, server};
    use super::server::ClientConnection;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    struct Server {
        counter: Arc<AtomicUsize>,
    }

    impl Service for Server {
        fn handle(&mut self, connection: &mut ClientConnection, packet: Packet, event_loop: &mut EventLoop<server::Dispatcher>) {
            connection.reply(event_loop, Packet {
                id: packet.id,
                payload: super::serialize(&(self.counter.load(Ordering::SeqCst) as u64)).unwrap()
            });
            self.counter.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Server {
        fn new() -> Server {
            Server { counter: Arc::new(AtomicUsize::new(0)) }
        }
    }

    #[test]
    fn handle() {
        let _ = env_logger::init();
        let server = Server::new();
        let serve_handle = server::Server::spawn("localhost:0", server).unwrap();
        let client = Client::spawn(serve_handle.local_addr()).unwrap();
        client.shutdown().unwrap();
        serve_handle.shutdown().unwrap();
    }

    #[test]
    fn simple() {
        let _ = env_logger::init();
        let server = Server::new();
        let count = server.counter.clone();
        let serve_handle = server::Server::spawn("localhost:0", server).unwrap();
        // The explicit type is required so that it doesn't deserialize a u32 instead of u64
        let client = Client::spawn(serve_handle.local_addr()).unwrap();
        assert_eq!(0u64, client.rpc_fut(&()).unwrap().get().unwrap());
        assert_eq!(1, count.load(Ordering::SeqCst));
        assert_eq!(1u64, client.rpc_fut(&()).unwrap().get().unwrap());
        assert_eq!(2, count.load(Ordering::SeqCst));
        client.shutdown().unwrap();
        serve_handle.shutdown().unwrap();
    }

    #[test]
    fn force_shutdown() {
        let _ = env_logger::init();
        let server = Server::new();
        let serve_handle = server::Server::spawn("localhost:0", server).unwrap();
        let client = Client::spawn(serve_handle.local_addr()).unwrap();
        let thread = thread::spawn(move || serve_handle.shutdown());
        info!("force_shutdown:: rpc1: {:?}", client.rpc_fut::<_, u64>(&()).unwrap().get().unwrap());
        thread.join().unwrap().unwrap();
    }

    #[test]
    fn client_failed_rpc() {
        let _ = env_logger::init();
        let server = Server::new();
        let serve_handle = server::Server::spawn("localhost:0", server).unwrap();
        let client = Client::spawn(serve_handle.local_addr()).unwrap();
        info!("Rpc 1");
        client.rpc_fut::<_, u64>(&()).unwrap().get().unwrap();
        info!("Shutting down server...");
        serve_handle.shutdown().unwrap();
        info!("Rpc 2");
        match client.rpc_fut::<_, u64>(&()).unwrap().get() {
            Err(::Error::ConnectionBroken) => {}
            otherwise => panic!("Expected Err(ConnectionBroken), got {:?}", otherwise),
        }
        info!("Rpc 3");
        if let Ok(..) = client.rpc_fut::<_, u64>(&()).unwrap().get() { // Test whether second failure hangs
            panic!("Should not be able to receive a successful rpc after ConnectionBroken.");
        }
        info!("Shutting down...");
        client.shutdown().unwrap();
    }

    #[test]
    fn async() {
        let _ = env_logger::init();
        let server = Server::new();
        let serve_handle = server::Server::spawn("localhost:0", server).unwrap();
        let client = Client::spawn(serve_handle.local_addr()).unwrap();

        // Drop future immediately; does the reader channel panic when sending?
        client.rpc_fut::<_, u64>(&()).unwrap().get().unwrap();
        // If the reader panicked, this won't succeed
        client.rpc_fut::<_, u64>(&()).unwrap().get().unwrap();

        client.shutdown().unwrap();
        serve_handle.shutdown().unwrap();
    }

    #[test]
    fn vec_serialization() {
        let v = vec![1, 2, 3, 4, 5];
        let serialized = super::serialize(&v).unwrap();
        assert_eq!(v, super::deserialize::<Vec<u8>>(&serialized).unwrap());
    }
}
