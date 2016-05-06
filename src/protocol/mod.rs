// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::{self, SizeLimit};
use bincode::serde::{deserialize_from, serialize_into, serialized_size};
use byteorder::{BigEndian, ByteOrder, ReadBytesExt};
use mio::*;
use mio::tcp::TcpStream;
use self::ReadState::*;
use self::WriteState::*;
use serde;
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::time::Duration;

pub mod client;
mod server;
mod packet;
/// Experimental mio-based async protocol.
pub mod async;

pub use self::packet::Packet;
pub use self::client::{Client, ClientHandle, Dispatcher, Future, SenderType};
pub use self::server::{Serve, ServeHandle};

quick_error! {
    /// Async errors.
    #[derive(Debug)]
    pub enum Error {
        ConnectionBroken {}
        /// IO error.
        Io(err: io::Error) {
            from()
            description(err.description())
        }
        Rx(err: mpsc::RecvError) {
            from()
            description(err.description())
        }
        /// Serialization error.
        Deserialize(err: bincode::serde::DeserializeError) {
            from()
            description(err.description())
        }
        Serialize(err: bincode::serde::SerializeError) {
            from()
            description(err.description())
        }
        DeregisterClient(err: NotifyError<()>) {
            from(DeregisterClientError)
            description(err.description())
        }
        RegisterClient(err: NotifyError<()>) {
            from(RegisterClientError)
            description(err.description())
        }
        DeregisterServer(err: NotifyError<()>) {
            from(DeregisterServerError)
            description(err.description())
        }
        RegisterServer(err: NotifyError<()>) {
            from(RegisterServerError)
            description(err.description())
        }
        Rpc(err: NotifyError<()>) {
            from(RpcError)
            description(err.description())
        }
        ShutdownClient(err: NotifyError<()>) {
            from(ShutdownClientError)
            description(err.description())
        }
        ShutdownServer(err: NotifyError<()>) {
            from(ShutdownServerError)
            description(err.description())
        }
        NoAddressFound {}
    }
}

struct RegisterServerError(NotifyError<async::server::Action>);
struct DeregisterServerError(NotifyError<async::server::Action>);
struct ShutdownServerError(NotifyError<async::server::Action>);
struct RegisterClientError(NotifyError<client::Action>);
struct DeregisterClientError(NotifyError<client::Action>);
struct ShutdownClientError(NotifyError<client::Action>);
struct RpcError(NotifyError<client::Action>);

macro_rules! from_err {
    ($from:ty, $to:expr) => {
        impl ::std::convert::From<$from> for Error {
            fn from(e: $from) -> Self {
                $to(discard_inner(e.0))
            }
        }
    }
}

from_err!(RegisterServerError, Error::RegisterServer);
from_err!(DeregisterServerError, Error::DeregisterServer);
from_err!(RegisterClientError, Error::RegisterClient);
from_err!(DeregisterClientError, Error::DeregisterClient);
from_err!(ShutdownClientError, Error::ShutdownClient);
from_err!(ShutdownServerError, Error::ShutdownServer);
from_err!(RpcError, Error::Rpc);

fn discard_inner<A>(e: NotifyError<A>) -> NotifyError<()> {
    match e {
        NotifyError::Io(e) => NotifyError::Io(e),
        NotifyError::Full(..) => NotifyError::Full(()),
        NotifyError::Closed(Some(..)) => NotifyError::Closed(None),
        NotifyError::Closed(None) => NotifyError::Closed(None),
    }
}

/// Configuration for client and server.
#[derive(Debug, Default)]
pub struct Config {
    /// Request/Response timeout between packet delivery.
    pub timeout: Option<Duration>,
}

/// Return type of rpc calls: either the successful return value, or a client error.
pub type Result<T> = ::std::result::Result<T, Error>;

trait Deserialize: Read + Sized {
    fn deserialize<T: serde::Deserialize>(&mut self) -> Result<Packet<T>> {
        let id = try!(deserialize_from::<_, u64>(self, SizeLimit::Infinite));
        let len = try!(deserialize_from::<_, u64>(self, SizeLimit::Infinite));
        debug!("Deserializing message of len {}", len);
        deserialize_from(self, SizeLimit::Infinite)
            .map_err(Error::from)
            .map(|msg| {
                Packet {
                    rpc_id: id,
                    message: msg,
                }
            })
    }
}

impl<R: Read> Deserialize for R {}

trait Serialize: Write + Sized {
    fn serialize<T: serde::Serialize>(&mut self, id: u64, value: &T) -> Result<()> {
        try!(serialize_into(self, &id, SizeLimit::Infinite));
        try!(serialize_into(self, &serialized_size(value), SizeLimit::Infinite));
        try!(serialize_into(self, value, SizeLimit::Infinite));
        try!(self.flush());
        Ok(())
    }
}

impl<W: Write> Serialize for W {}

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

impl WriteState {
    fn next(state: &mut Option<WriteState>,
            socket: &mut TcpStream,
            outbound: &mut VecDeque<client::Packet>,
            interest: &mut EventSet,
            token: Token) {
        let update = match *state {
            None => {
                match outbound.pop_front() {
                    Some(packet) => {
                        let size = packet.payload.len() as u64;
                        info!("Packet: id: {}, size: {}, paylod: {:?}",
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
                        interest.remove(EventSet::writable());
                        None
                    }
                }
            }
            Some(WriteId { ref mut written, mut id, size, ref mut payload }) => {
                match socket.try_write(&mut id[*written as usize..]) {
                    Ok(None) => {
                        debug!("Client {:?}: spurious wakeup while writing id.", token);
                        None
                    }
                    Ok(Some(bytes_written)) => {
                        debug!("Client {:?}: wrote {} bytes of id.", token, bytes_written);
                        *written += bytes_written as u8;
                        if *written == 8 {
                            debug!("Client {:?}: done writing id.", token);
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
                        debug!("Client {:?}: write err, {:?}", token, e);
                        interest.remove(EventSet::writable());
                        Some(None)
                    }
                }
            }
            Some(WriteSize { ref mut written, mut size, ref mut payload }) => {
                match socket.try_write(&mut size[*written as usize..]) {
                    Ok(None) => {
                        debug!("Client {:?}: spurious wakeup while writing size.", token);
                        None
                    }
                    Ok(Some(bytes_written)) => {
                        debug!("Client {:?}: wrote {} bytes of size.", token, bytes_written);
                        *written += bytes_written as u8;
                        if *written == 8 {
                            debug!("Client {:?}: done writing size.", token);
                            Some(Some(WriteData(payload.split_off(0))))
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        debug!("Client {:?}: write err, {:?}", token, e);
                        interest.remove(EventSet::writable());
                        Some(None)
                    }
                }
            }
            Some(WriteData(ref mut buf)) => {
                match socket.try_write(buf) {
                    Ok(None) => {
                        debug!("Client flushing buf; WOULDBLOCK");
                        None
                    }
                    Ok(Some(written)) => {
                        debug!("Client wrote {} bytes of payload.", written);
                        *buf = buf.split_off(written);
                        if buf.is_empty() {
                            debug!("Client finished writing;");
                            interest.insert(EventSet::readable());
                            debug!("Remaining interests: {:?}", interest);
                            Some(None)
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        debug!("Client write error: {:?}", e);
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

    fn next<F>(state: &mut ReadState,
               socket: &mut TcpStream,
               handler: F,
               interest: &mut EventSet,
               token: Token) 
        where F: FnOnce(client::Packet)
    {
        let update = match *state {
            ReadId { ref mut read, ref mut buf } => {
                debug!("{:?}: reading id.", token);
                match socket.try_read(&mut buf[*read as usize..]) {
                    Ok(None) => {
                        debug!("{:?}: spurious wakeup while reading id.", token);
                        None
                    }
                    Ok(Some(bytes_read)) => {
                        debug!("{:?}: read {} bytes of id.", token, bytes_read);
                        *read += bytes_read as u8;
                        if *read == 8 {
                            let id = (buf as &[u8]).read_u64::<BigEndian>().unwrap();
                            debug!("{:?}: read id {}.", token, id);
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
                        debug!("{:?}: read err, {:?}", token, e);
                        interest.remove(EventSet::readable());
                        None
                    }
                }
            }
            ReadSize { id, ref mut read, ref mut buf } => {
                match socket.try_read(&mut buf[*read as usize..]) {
                    Ok(None) => {
                        debug!("{:?}: spurious wakeup while reading size.", token);
                        None
                    }
                    Ok(Some(bytes_read)) => {
                        debug!("{:?}: read {} bytes of size.", token, bytes_read);
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
                        debug!("{:?}: read err, {:?}", token, e);
                        interest.remove(EventSet::readable());
                        None
                    }
                }
            }
            ReadData { id, message_len, ref mut read, ref mut buf } => {
                match socket.try_read(&mut buf[*read..]) {
                    Ok(None) => {
                        debug!("{:?}: spurious wakeup while reading data.", token);
                        None
                    }
                    Ok(Some(bytes_read)) => {
                        *read += bytes_read;
                        debug!("{:?}: read {} more bytes of data for a total of {}; {} \
                                needed",
                               token,
                               bytes_read,
                               *read,
                               message_len);
                        if *read == message_len {
                            let payload = buf.split_off(0);
                            handler(client::Packet { id: id, payload: payload });
                            Some(ReadState::init())
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        debug!("{:?}: read err, {:?}", token, e);
                        interest.remove(EventSet::readable());
                        None
                    }
                }
            }
        };
        if let Some(next) = update {
            *state = next;
        }
    }
}

#[cfg(test)]
mod test {
    extern crate env_logger;
    use super::{Client, Config, Serve};
    use scoped_pool::Pool;
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use std::time::Duration;

    fn test_timeout() -> Option<Duration> {
        Some(Duration::from_secs(1))
    }

    struct Server {
        counter: Mutex<u64>,
    }

    impl Serve for Server {
        type Request = ();
        type Reply = u64;

        fn serve(&self, _: ()) -> u64 {
            let mut counter = self.counter.lock().unwrap();
            let reply = *counter;
            *counter += 1;
            reply
        }
    }

    impl Server {
        fn new() -> Server {
            Server { counter: Mutex::new(0) }
        }

        fn count(&self) -> u64 {
            *self.counter.lock().unwrap()
        }
    }

    #[test]
    fn handle() {
        let _ = env_logger::init();
        let server = Arc::new(Server::new());
        let serve_handle = server.spawn("localhost:0").unwrap();
        let client = Client::dial(serve_handle.dialer()).unwrap();
        client.shutdown().unwrap();
        serve_handle.shutdown();
    }

    #[test]
    fn simple() {
        let _ = env_logger::init();
        let server = Arc::new(Server::new());
        let serve_handle = server.clone().spawn("localhost:0").unwrap();
        // The explicit type is required so that it doesn't deserialize a u32 instead of u64
        let client = Client::dial(serve_handle.dialer()).unwrap();
        assert_eq!(0u64, client.rpc(&()).unwrap().get().unwrap());
        assert_eq!(1, server.count());
        assert_eq!(1u64, client.rpc(&()).unwrap().get().unwrap());
        assert_eq!(2, server.count());
        client.shutdown().unwrap();
        serve_handle.shutdown();
    }

    struct BarrierServer {
        barrier: Barrier,
        inner: Server,
    }

    impl Serve for BarrierServer {
        type Request = ();
        type Reply = u64;
        fn serve(&self, request: ()) -> u64 {
            self.barrier.wait();
            self.inner.serve(request)
        }
    }

    impl BarrierServer {
        fn new(n: usize) -> BarrierServer {
            BarrierServer {
                barrier: Barrier::new(n),
                inner: Server::new(),
            }
        }

        fn count(&self) -> u64 {
            self.inner.count()
        }
    }

    #[test]
    fn force_shutdown() {
        let _ = env_logger::init();
        let server = Arc::new(Server::new());
        let serve_handle = server.spawn_with_config("localhost:0",
                                                    Config { timeout: Some(Duration::new(0, 10)) })
                                 .unwrap();
        let client = Client::dial(serve_handle.dialer()).unwrap();
        let thread = thread::spawn(move || serve_handle.shutdown());
        info!("force_shutdown:: rpc1: {:?}", client.rpc::<_, u64>(&()).unwrap().get().unwrap());
        thread.join().unwrap();
    }

    #[test]
    fn client_failed_rpc() {
        let _ = env_logger::init();
        let server = Arc::new(Server::new());
        let serve_handle = server.spawn_with_config("localhost:0",
                                                    Config { timeout: test_timeout() })
                                 .unwrap();
        let client = Client::dial(serve_handle.dialer()).unwrap();
        client.rpc::<_, u64>(&()).unwrap().get().unwrap();
        serve_handle.shutdown();
        info!("Rpc 2");
        match client.rpc::<_, u64>(&()).unwrap().get() {
            Err(super::Error::ConnectionBroken) => {}
            otherwise => panic!("Expected Err(ConnectionBroken), got {:?}", otherwise),
        }
        info!("Rpc 3");
        if let Ok(..) = client.rpc::<_, u64>(&()).unwrap().get() { // Test whether second failure hangs
            panic!("Should not be able to receive a successful rpc after ConnectionBroken.");
        }
        info!("Shutting down...");
        client.shutdown().unwrap();
    }

    #[test]
    fn concurrent() {
        let _ = env_logger::init();
        let concurrency = 10;
        let pool = Pool::new(concurrency);
        let server = Arc::new(BarrierServer::new(concurrency));
        let serve_handle = server.clone().spawn("localhost:0").unwrap();
        let client = Client::dial(serve_handle.dialer()).unwrap();
        pool.scoped(|scope| {
            for _ in 0..concurrency {
                let client = client.clone();
                scope.execute(move || {
                    client.rpc::<_, u64>(&()).unwrap().get().unwrap();
                });
            }
        });
        assert_eq!(concurrency as u64, server.count());
        client.shutdown().unwrap();
        serve_handle.shutdown();
    }

    #[test]
    fn async() {
        let _ = env_logger::init();
        let server = Arc::new(Server::new());
        let serve_handle = server.spawn("localhost:0").unwrap();
        let client = Client::dial(serve_handle.dialer()).unwrap();

        // Drop future immediately; does the reader channel panic when sending?
        client.rpc::<_, u64>(&()).unwrap().get().unwrap();
        // If the reader panicked, this won't succeed
        client.rpc::<_, u64>(&()).unwrap().get().unwrap();

        client.shutdown().unwrap();
        serve_handle.shutdown();
    }
}
