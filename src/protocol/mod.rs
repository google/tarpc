// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::SizeLimit;
use bincode::serde as bincode;
use byteorder::{BigEndian, ByteOrder, ReadBytesExt};
use mio::*;
use mio::tcp::TcpStream;
use self::ReadState::*;
use self::WriteState::*;
use serde;
use std::collections::VecDeque;
use std::io::{self, Cursor};
use std::mem;

/// Client-side implementation of the tarpc protocol.
pub mod client;
/// Server-side implementation of the tarpc protocol.
pub mod server;

pub use self::client::{Client, ClientHandle, Future, SenderType};
pub use self::server::{ServeHandle, Server, Service};

/// The means of communication between client and server.
pub struct Packet {
    /// Identifies the request. The reply packet should specify the same id as the request.
    pub id: u64,
    /// The payload is typically a message that the client and server deserializes
    /// before handling.
    pub payload: Vec<u8>,
}

trait Data {
    type Read;

    fn len(&self) -> usize;
    fn range_from(&self, from: usize) -> &[u8];
    fn range_from_mut(&mut self, from: usize) -> &mut [u8];
    fn read(&mut self) -> Self::Read;
}

impl Data for Vec<u8> {
    type Read = Self;

    #[inline]
    fn len(&self) -> usize {
        self.len()
    }

    #[inline]
    fn range_from(&self, from: usize) -> &[u8] {
        &self[from..]
    }

    #[inline]
    fn range_from_mut(&mut self, from: usize) -> &mut [u8] {
        &mut self[from..]
    }

    #[inline]
    fn read(&mut self) -> Self {
        mem::replace(self, vec![])
    }
}

impl Data for [u8; 8] {
    type Read = u64;

    #[inline]
    fn len(&self) -> usize {
        8
    }

    #[inline]
    fn range_from(&self, from: usize) -> &[u8] {
        &self[from..]
    }

    #[inline]
    fn range_from_mut(&mut self, from: usize) -> &mut [u8] {
        &mut self[from..]
    }

    #[inline]
    fn read(&mut self) -> u64 {
        (self as &[u8]).read_u64::<BigEndian>().unwrap()
    }
}

enum NextWriteAction {
    Stop,
    Continue,
}

struct Writer<D>
    where D: Data
{
    written: usize,
    data: D,
}

impl<D> Writer<D>
    where D: Data
{
    /// Writes data to stream. Returns Ok(true) if all data has been written or Ok(false) if
    /// there's still data to write.
    fn try_write(&mut self, stream: &mut TcpStream) -> io::Result<NextWriteAction> {
        match try!(stream.try_write(&mut self.data.range_from(self.written))) {
            None => {
                debug!("Writer: spurious wakeup, {}/{}",
                       self.written,
                       self.data.len());
                Ok(NextWriteAction::Continue)
            }
            Some(bytes_written) => {
                debug!("Writer: wrote {} bytes of {} remaining.",
                       bytes_written,
                       self.data.len() - self.written);
                self.written += bytes_written;
                if self.written == self.data.len() {
                    Ok(NextWriteAction::Stop)
                } else {
                    Ok(NextWriteAction::Continue)
                }
            }
        }
    }
}

type U64Writer = Writer<[u8; 8]>;

impl U64Writer {
    fn empty() -> U64Writer {
        Writer {
            written: 0,
            data: [0; 8],
        }
    }

    fn from_u64(data: u64) -> Self {
        let mut buf = [0; 8];
        BigEndian::write_u64(&mut buf[..], data);

        Writer {
            written: 0,
            data: buf,
        }
    }
}

type VecWriter = Writer<Vec<u8>>;

impl VecWriter {
    fn from_vec(data: Vec<u8>) -> Self {
        Writer {
            written: 0,
            data: data,
        }
    }
}

/// A state machine that writes packets in non-blocking fashion.
enum WriteState {
    WriteId {
        id: U64Writer,
        size: U64Writer,
        payload: Option<VecWriter>,
    },
    WriteSize {
        size: U64Writer,
        payload: Option<VecWriter>,
    },
    WriteData(VecWriter),
}

enum NextWriteState {
    Same,
    Nothing,
    Next(WriteState),
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
                        debug!("WriteState {:?}: Packet: id: {}, size: {}",
                               token,
                               packet.id,
                               size);
                        NextWriteState::Next(WriteState::WriteId {
                            id: U64Writer::from_u64(packet.id),
                            size: U64Writer::from_u64(size),
                            payload: if packet.payload.is_empty() {
                                None
                            } else {
                                Some(VecWriter::from_vec(packet.payload))
                            },
                        })
                    }
                    None => {
                        interest.remove(EventSet::writable());
                        NextWriteState::Same
                    }
                }
            }
            Some(WriteId { ref mut id, ref mut size, ref mut payload }) => {
                match id.try_write(socket) {
                    Ok(NextWriteAction::Stop) => {
                        debug!("WriteId {:?}: transitioning to writing size", token);
                        let size = mem::replace(size, U64Writer::empty());
                        let payload = mem::replace(payload, None);
                        NextWriteState::Next(WriteState::WriteSize {
                            size: size,
                            payload: payload,
                        })
                    }
                    Ok(NextWriteAction::Continue) => NextWriteState::Same,
                    Err(e) => {
                        debug!("WriteId {:?}: write err, {:?}", token, e);
                        NextWriteState::Nothing
                    }
                }
            }
            Some(WriteSize { ref mut size, ref mut payload }) => {
                match size.try_write(socket) {
                    Ok(NextWriteAction::Stop) => {
                        let payload = mem::replace(payload, None);
                        if let Some(payload) = payload {
                            debug!("WriteSize {:?}: Transitioning to writing payload", token);
                            NextWriteState::Next(WriteState::WriteData(payload))
                        } else {
                            debug!("WriteSize {:?}: no payload to write.", token);
                            NextWriteState::Nothing
                        }
                    }
                    Ok(NextWriteAction::Continue) => NextWriteState::Same,
                    Err(e) => {
                        debug!("WriteSize {:?}: write err, {:?}", token, e);
                        NextWriteState::Nothing
                    }
                }
            }
            Some(WriteData(ref mut payload)) => {
                match payload.try_write(socket) {
                    Ok(NextWriteAction::Stop) => {
                        debug!("WriteData {:?}: done writing payload", token);
                        NextWriteState::Nothing
                    }
                    Ok(NextWriteAction::Continue) => NextWriteState::Same,
                    Err(e) => {
                        debug!("WriteData {:?}: write err, {:?}", token, e);
                        NextWriteState::Nothing
                    }
                }
            }
        };
        match update {
            NextWriteState::Next(next) => *state = Some(next),
            NextWriteState::Nothing => {
                *state = None;
                debug!("WriteSize {:?}: Done writing.", token);
                if outbound.is_empty() {
                    interest.remove(EventSet::writable());
                }
                interest.insert(EventSet::readable());
                debug!("Remaining interests: {:?}", interest);
            }
            NextWriteState::Same => {}
        }
    }
}

struct Reader<D>
    where D: Data
{
    read: usize,
    data: D,
}

enum NextReadAction<D>
    where D: Data
{
    Continue,
    Stop(D::Read),
}

impl<D> Reader<D>
    where D: Data
{
    fn try_read(&mut self, stream: &mut TcpStream) -> io::Result<NextReadAction<D>> {
        match try!(stream.try_read(self.data.range_from_mut(self.read))) {
            None => {
                debug!("Reader: spurious wakeup, {}/{}", self.read, self.data.len());
                Ok(NextReadAction::Continue)
            }
            Some(bytes_read) => {
                debug!("Reader: read {} bytes of {} remaining.",
                       bytes_read,
                       self.data.len() - self.read);
                self.read += bytes_read;
                if self.read == self.data.len() {
                    trace!("Reader: finished.");
                    Ok(NextReadAction::Stop(self.data.read()))
                } else {
                    trace!("Reader: not finished.");
                    Ok(NextReadAction::Continue)
                }
            }
        }
    }
}

type U64Reader = Reader<[u8; 8]>;

impl U64Reader {
    fn new() -> Self {
        Reader {
            read: 0,
            data: [0; 8],
        }
    }
}

type VecReader = Reader<Vec<u8>>;

impl VecReader {
    fn with_len(len: usize) -> Self {
        VecReader {
            read: 0,
            data: vec![0; len],
        }
    }
}

/// A state machine that reads packets in non-blocking fashion.
enum ReadState {
    /// Tracks how many bytes of the message ID have been read.
    ReadId(U64Reader),
    /// Tracks how many bytes of the message size have been read.
    ReadLen {
        id: u64,
        len: U64Reader,
    },
    /// Tracks read progress.
    ReadData {
        /// ID of the message being read.
        id: u64,
        /// Reads the bufer.
        buf: VecReader,
    },
}

enum NextReadState {
    Same,
    Next(ReadState),
    Reset(Packet),
}

impl ReadState {
    fn init() -> ReadState {
        ReadId(U64Reader::new())
    }

    fn next(state: &mut ReadState, socket: &mut TcpStream, token: Token) -> Option<Packet> {
        let next = match *state {
            ReadId(ref mut reader) => {
                debug!("ReadState {:?}: reading id.", token);
                match reader.try_read(socket) {
                    Ok(NextReadAction::Continue) => NextReadState::Same,
                    Ok(NextReadAction::Stop(id)) => {
                        debug!("ReadId {:?}: transitioning to reading len.", token);
                        NextReadState::Next(ReadLen {
                            id: id,
                            len: U64Reader::new(),
                        })
                    }
                    Err(e) => {
                        // TODO(tikue): handle this better?
                        debug!("ReadState {:?}: read err, {:?}", token, e);
                        NextReadState::Same
                    }
                }
            }
            ReadLen { id, ref mut len } => {
                match len.try_read(socket) {
                    Ok(NextReadAction::Continue) => NextReadState::Same,
                    Ok(NextReadAction::Stop(len)) => {
                        debug!("ReadLen: message len = {}", len);
                        if len == 0 {
                            debug!("Reading complete.");
                            NextReadState::Reset(Packet {
                                id: id,
                                payload: vec![],
                            })
                        } else {
                            debug!("ReadLen {:?}: transitioning to reading payload.", token);
                            NextReadState::Next(ReadData {
                                id: id,
                                buf: VecReader::with_len(len as usize),
                            })
                        }
                    }
                    Err(e) => {
                        debug!("ReadState {:?}: read err, {:?}", token, e);
                        NextReadState::Same
                    }
                }
            }
            ReadData { id, ref mut buf } => {
                match buf.try_read(socket) {
                    Ok(NextReadAction::Continue) => NextReadState::Same,
                    Ok(NextReadAction::Stop(payload)) => {
                        NextReadState::Reset(Packet {
                            id: id,
                            payload: payload,
                        })
                    }
                    Err(e) => {
                        debug!("ReadState {:?}: read err, {:?}", token, e);
                        NextReadState::Same
                    }
                }
            }
        };
        match next {
            NextReadState::Same => None,
            NextReadState::Next(next) => {
                *state = next;
                None
            }
            NextReadState::Reset(packet) => {
                *state = ReadState::init();
                Some(packet)
            }
        }
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
        fn handle(&mut self,
                  connection: &mut ClientConnection,
                  packet: Packet,
                  event_loop: &mut EventLoop<server::Dispatcher>) {
            connection.reply(event_loop,
                             Packet {
                                 id: packet.id,
                                 payload:
                                     super::serialize(&(self.counter.load(Ordering::SeqCst) as u64))
                                         .unwrap(),
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
        info!("force_shutdown:: rpc1: {:?}",
              client.rpc_fut::<_, u64>(&()).unwrap().get().unwrap());
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
        if let Ok(..) = client.rpc_fut::<_, u64>(&()).unwrap().get() {
            // Test whether second failure hangs
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
        info!("Rpc 1: {}",
              client.rpc_fut::<_, u64>(&()).unwrap().get().unwrap());
        // If the reader panicked, this won't succeed
        info!("Rpc 2: {}",
              client.rpc_fut::<_, u64>(&()).unwrap().get().unwrap());

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
