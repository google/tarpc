// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::{self, SizeLimit};
use bincode::serde::{deserialize_from, serialize_into, serialized_size};
use mio::NotifyError;
use serde;
use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::time::Duration;

mod client;
mod server;
mod packet;
/// Experimental mio-based async protocol.
pub mod async;

pub use self::packet::Packet;
pub use self::client::{Action, Client, ClientHandle, Dispatcher, Future, SenderType, WriteState};
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
        Deregister(err: NotifyError<()>) {
            from(DeregisterError)
            description(err.description())
        }
        Register(err: NotifyError<()>) {
            from(RegisterError)
            description(err.description())
        }
        Rpc(err: NotifyError<()>) {
            from(RpcError)
            description(err.description())
        }
        Shutdown(err: NotifyError<()>) {
            from(ShutdownError)
            description(err.description())
        }
        NoAddressFound {}
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
