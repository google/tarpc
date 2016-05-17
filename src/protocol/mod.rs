// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::SizeLimit;
use bincode::serde as bincode;
use byteorder::{BigEndian, ReadBytesExt};
use serde;
use std::io::Cursor;
use std::mem;

mod reader;
mod writer;

type ReadState = self::reader::ReadState;
type WriteState = self::writer::WriteState;

/// AsyncClient-side implementation of the tarpc protocol.
pub mod client;
/// AsyncServer-side implementation of the tarpc protocol.
pub mod server;

pub use self::client::{AsyncClient, ClientHandle, Future, SenderType};
pub use self::server::{AsyncServer, AsyncService, ServeHandle};

/// The means of communication between client and server.
#[derive(Debug)]
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
    use super::{AsyncClient, AsyncService, Packet, server};
    use super::server::ClientConnection;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    #[derive(Debug)]
    struct AsyncServer {
        counter: Arc<AtomicUsize>,
    }

    impl AsyncService for AsyncServer {
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

    impl AsyncServer {
        fn new() -> AsyncServer {
            AsyncServer { counter: Arc::new(AtomicUsize::new(0)) }
        }
    }

    #[test]
    fn simple() {
        let _ = env_logger::init();
        let server = AsyncServer::new();
        let count = server.counter.clone();
        let serve_handle = server::AsyncServer::spawn("localhost:0", server).unwrap();
        // The explicit type is required so that it doesn't deserialize a u32 instead of u64
        let client = AsyncClient::connect(serve_handle.local_addr()).unwrap();
        assert_eq!(0u64, client.rpc_fut(&()).unwrap().get().unwrap());
        assert_eq!(1, count.load(Ordering::SeqCst));
        assert_eq!(1u64, client.rpc_fut(&()).unwrap().get().unwrap());
        assert_eq!(2, count.load(Ordering::SeqCst));
    }

    #[test]
    fn force_shutdown() {
        let _ = env_logger::init();
        let server = server::AsyncServer::new("localhost:0", AsyncServer::new()).unwrap();
        let registry = server::Dispatcher::spawn().unwrap();
        let serve_handle = registry.clone().register(server).unwrap();
        let client = AsyncClient::connect(serve_handle.local_addr()).unwrap();
        let thread = thread::spawn(move || registry.shutdown());
        info!("force_shutdown:: rpc1: {:?}",
              client.rpc_fut::<_, u64>(&()).unwrap().get().unwrap());
        thread.join().unwrap().unwrap();
    }

    #[test]
    fn client_failed_rpc() {
        let _ = env_logger::init();
        let server = server::AsyncServer::new("localhost:0", AsyncServer::new()).unwrap();
        let registry = server::Dispatcher::spawn().unwrap();
        let serve_handle = registry.clone().register(server).unwrap();
        let client = AsyncClient::connect(serve_handle.local_addr()).unwrap();
        info!("Rpc 1");
        client.rpc_fut::<_, u64>(&()).unwrap().get().unwrap();
        info!("Shutting down server...");
        registry.shutdown().unwrap();
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
    }

    #[test]
    fn async() {
        let _ = env_logger::init();
        let server = AsyncServer::new();
        let serve_handle = server::AsyncServer::spawn("localhost:0", server).unwrap();
        let client = AsyncClient::connect(serve_handle.local_addr()).unwrap();

        // Drop future immediately; does the reader channel panic when sending?
        info!("Rpc 1: {}",
              client.rpc_fut::<_, u64>(&()).unwrap().get().unwrap());
        // If the reader panicked, this won't succeed
        info!("Rpc 2: {}",
              client.rpc_fut::<_, u64>(&()).unwrap().get().unwrap());
    }

    #[test]
    fn vec_serialization() {
        let v = vec![1, 2, 3, 4, 5];
        let serialized = super::serialize(&v).unwrap();
        assert_eq!(v, super::deserialize::<Vec<u8>>(&serialized).unwrap());
    }
}
