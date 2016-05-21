// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::SizeLimit;
use bincode::serde as bincode;
use serde;
use std::io::Cursor;

mod reader;
mod writer;

type ReadState = self::reader::ReadState;
type WriteState<D> = self::writer::WriteState<D>;

/// AsyncClient-side implementation of the tarpc protocol.
pub mod client;
/// AsyncServer-side implementation of the tarpc protocol.
pub mod server;

pub use self::client::{AsyncClient, ClientHandle, Future};
pub use self::server::{AsyncServer, AsyncService, ServeHandle};

pub use self::reader::Read;
pub use self::writer::Write;

/// The means of communication between client and server.
#[derive(Clone, Debug)]
pub struct Packet<D> {
    /// Identifies the request. The reply packet should specify the same id as the request.
    pub id: u64,
    /// The payload is typically a message that the client and server deserializes
    /// before handling.
    pub payload: D,
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
    use super::{AsyncClient, AsyncService, server};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct AsyncServer {
        counter: Arc<AtomicUsize>,
    }

    impl AsyncService for AsyncServer {
        fn handle(&mut self, ctx: ::Ctx, _: Vec<u8>) {
            ctx.reply(Ok(self.counter.load(Ordering::SeqCst) as u64)).unwrap();
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
        let serve_handle = server::AsyncServer::listen("localhost:0",
                                                       server,
                                                       ::server::Config::default())
                               .expect(pos!());
        // The explicit type is required so that it doesn't deserialize a u32 instead of u64
        let client = AsyncClient::connect(serve_handle.local_addr()).expect(pos!());
        assert_eq!(0u64, client.rpc_sync(&()).expect(pos!()));
        assert_eq!(1, count.load(Ordering::SeqCst));
        assert_eq!(1u64, client.rpc_sync(&()).expect(pos!()));
        assert_eq!(2, count.load(Ordering::SeqCst));
    }

    #[test]
    fn client_failed_rpc() {
        let _ = env_logger::init();
        let server = server::AsyncServer::new("localhost:0", AsyncServer::new()).unwrap();
        let registry = server::Dispatcher::spawn().unwrap();
        let serve_handle = registry.register(server).unwrap();
        let client = AsyncClient::connect(serve_handle.local_addr()).unwrap();
        info!("Rpc 1");
        client.rpc_sync::<_, u64>(&()).unwrap();
        info!("Shutting down server...");
        registry.shutdown().unwrap();
        info!("Rpc 2");
        match client.rpc_sync::<_, u64>(&()) {
            Err(::Error::ConnectionBroken) => {}
            otherwise => panic!("Expected Err(ConnectionBroken), got {:?}", otherwise),
        }
        info!("Rpc 3");
        if let Ok(..) = client.rpc_sync::<_, u64>(&()) {
            // Test whether second failure hangs
            panic!("Should not be able to receive a successful rpc after ConnectionBroken.");
        }
    }

    #[test]
    fn async() {
        let _ = env_logger::init();
        let server = AsyncServer::new();
        let serve_handle = server::AsyncServer::listen("localhost:0",
                                                       server,
                                                       ::server::Config::default())
                               .unwrap();
        let client = AsyncClient::connect(serve_handle.local_addr()).unwrap();

        // Drop future immediately; does the reader channel panic when sending?
        info!("Rpc 1: {}", client.rpc_sync::<_, u64>(&()).unwrap());
        // If the reader panicked, this won't succeed
        info!("Rpc 2: {}", client.rpc_sync::<_, u64>(&()).unwrap());
    }

    #[test]
    fn vec_serialization() {
        let v = vec![1, 2, 3, 4, 5];
        let serialized = super::serialize(&v).unwrap();
        assert_eq!(v, super::deserialize::<Vec<u8>>(&serialized).unwrap());
    }

    #[test]
    fn deregister() {
        use client;
        use server::{self, AsyncServer, AsyncService};
        #[derive(Debug)]
        struct NoopServer;

        impl AsyncService for NoopServer {
            fn handle(&mut self, ctx: ::Ctx, _: Vec<u8>) {
                ctx.reply(Ok(())).unwrap();
            }
        }

        let _ = env_logger::init();
        let server_registry = server::Dispatcher::spawn().unwrap();
        let serve_handle = server_registry.register(AsyncServer::new("localhost:0", NoopServer)
                                                        .unwrap())
                                          .expect(pos!());

        let client_registry = client::Dispatcher::spawn().unwrap();
        let client = client_registry.register(serve_handle.local_addr()).expect(pos!());
        assert_eq!(client_registry.debug().unwrap().clients, 1);
        let client2 = client.clone();
        assert_eq!(client_registry.debug().unwrap().clients, 1);
        drop(client2);
        assert_eq!(client_registry.debug().unwrap().clients, 1);
        drop(client);
        assert_eq!(client_registry.debug().unwrap().clients, 0);

        assert_eq!(server_registry.debug().unwrap().services, 1);
        let handle2 = serve_handle.clone();
        assert_eq!(server_registry.debug().unwrap().services, 1);
        drop(handle2);
        assert_eq!(server_registry.debug().unwrap().services, 1);
        drop(serve_handle);
        assert_eq!(server_registry.debug().unwrap().services, 0);

    }
}
