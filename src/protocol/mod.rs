// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::SizeLimit;
use bincode::serde as bincode;
use serde;
use std::io::{self, Cursor};
use std::mem;
use std::rc::Rc;

/// Client-side implementation of the tarpc protocol.
pub mod client;

/// Server-side implementation of the tarpc protocol.
pub mod server;

mod reader;
mod writer;

id_wrapper!(RpcId);

/// A helper trait to provide the map_non_block function on Results.
trait MapNonBlock<T> {
    /// Maps a `Result<T>` to a `Result<Option<T>>` by converting
    /// operation-would-block errors into `Ok(None)`.
    fn map_non_block(self) -> io::Result<Option<T>>;
}

impl<T> MapNonBlock<T> for io::Result<T> {
    fn map_non_block(self) -> io::Result<Option<T>> {
        use std::io::ErrorKind::WouldBlock;

        match self {
            Ok(value) => Ok(Some(value)),
            Err(err) => {
                if let WouldBlock = err.kind() {
                    Ok(None)
                } else {
                    Err(err)
                }
            }
        }
    }
}

/// Something that can tell you its length.
trait Len {
    /// The length of the container.
    fn len(&self) -> usize;
}

impl<L> Len for Rc<L>
    where L: Len
{
    #[inline]
    fn len(&self) -> usize {
        (&**self).len()
    }
}

impl Len for Vec<u8> {
    #[inline]
    fn len(&self) -> usize {
        self.len()
    }
}

impl Len for [u8; 8] {
    #[inline]
    fn len(&self) -> usize {
        8
    }
}

/// Serialize `s`, left-padding with 16 bytes.
///
/// Returns `Vec<u8>` if successful, otherwise `tarpc::Error`.
pub fn serialize<S: serde::Serialize>(s: &S) -> ::Result<Vec<u8>> {
    let mut buf = vec![0; 16];
    try!(bincode::serialize_into(&mut buf, s, SizeLimit::Infinite));
    Ok(buf)
}

/// Deserialize a buffer into a `D` and its ID. On error, returns `tarpc::Error`.
pub fn deserialize<D: serde::Deserialize>(buf: &[u8]) -> ::Result<D> {
    bincode::deserialize_from(&mut Cursor::new(&buf[mem::size_of::<u64>()..]),
                              SizeLimit::Infinite)
        .map_err(|e| e.into())
}

#[cfg(test)]
mod test {
    extern crate env_logger;
    use ::client::AsyncClient;
    use ::server::{self, AsyncService, GenericCtx};
    use std::mem;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct AsyncServer {
        counter: Arc<AtomicUsize>,
    }

    impl AsyncService for AsyncServer {
        fn handle(&self, ctx: GenericCtx, _: Vec<u8>) {
            ctx.for_type::<u64>().reply(Ok(self.counter.load(Ordering::SeqCst) as u64)).unwrap();
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
        let serve_handle =
            server::AsyncServer::listen("localhost:0", server, ::server::Config::default())
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
        let serve_handle = registry.clone().register(server).unwrap();
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
        let serve_handle =
            server::AsyncServer::listen("localhost:0", server, ::server::Config::default())
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
        assert_eq!(v,
                   super::deserialize::<Vec<u8>>(&serialized[mem::size_of::<u64>()..]).unwrap());
    }

    #[test]
    fn deregister() {
        use client;
        use server::{self, AsyncServer, AsyncService};
        #[derive(Debug)]
        struct NoopServer;

        impl AsyncService for NoopServer {
            fn handle(&self, ctx: GenericCtx, _: Vec<u8>) {
                ctx.for_type::<()>().reply(Ok(())).unwrap();
            }
        }

        let _ = env_logger::init();
        let server_registry = server::Dispatcher::spawn().unwrap();
        let serve_handle = server_registry.clone()
            .register(AsyncServer::new("localhost:0", NoopServer).unwrap())
            .expect(pos!());

        let client_registry = client::Dispatcher::spawn().unwrap();
        let client = client_registry.clone().connect(serve_handle.local_addr()).expect(pos!());
        assert_eq!(client_registry.debug().unwrap().clients, 1);
        let client2 = client.clone();
        assert_eq!(client_registry.debug().unwrap().clients, 1);
        drop(client2);
        assert_eq!(client_registry.debug().unwrap().clients, 1);
        drop(client);
        assert_eq!(client_registry.debug().unwrap().clients, 0);

        assert_eq!(server_registry.debug().unwrap().servers, 1);
        let handle2 = serve_handle.clone();
        assert_eq!(server_registry.debug().unwrap().servers, 1);
        drop(handle2);
        assert_eq!(server_registry.debug().unwrap().servers, 1);
        drop(serve_handle);
        assert_eq!(server_registry.debug().unwrap().servers, 0);

    }
}
