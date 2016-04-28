// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::{self, SizeLimit};
use bincode::serde::{deserialize_from, serialize_into, serialized_size};
use serde;
use serde::de::value::Error::EndOfStream;
use std::io::{self, Read, Write};
use std::convert;
use std::sync::Arc;
use std::time::Duration;

mod client;
mod server;
mod packet;
/// Experimental mio-based async protocol.
pub mod async;

pub use self::packet::Packet;
pub use self::client::{Client, Future};
pub use self::server::{Serve, ServeHandle};

/// Client errors that can occur during rpc calls
#[derive(Debug, Clone)]
pub enum Error {
    /// An IO-related error
    Io(Arc<io::Error>),
    /// The server hung up.
    ConnectionBroken,
}

impl convert::From<bincode::serde::SerializeError> for Error {
    fn from(err: bincode::serde::SerializeError) -> Error {
        match err {
            bincode::serde::SerializeError::IoError(err) => Error::Io(Arc::new(err)),
            err => panic!("Unexpected error during serialization: {:?}", err),
        }
    }
}

impl convert::From<bincode::serde::DeserializeError> for Error {
    fn from(err: bincode::serde::DeserializeError) -> Error {
        match err {
            bincode::serde::DeserializeError::Serde(EndOfStream) => Error::ConnectionBroken,
            bincode::serde::DeserializeError::IoError(err) => {
                match err.kind() {
                    io::ErrorKind::ConnectionReset |
                    io::ErrorKind::UnexpectedEof => Error::ConnectionBroken,
                    _ => Error::Io(Arc::new(err)),
                }
            }
            err => panic!("Unexpected error during deserialization: {:?}", err),
        }
    }
}

impl convert::From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(Arc::new(err))
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
    fn deserialize<T: serde::Deserialize>(&mut self) -> Result<T> {
        let len = try!(deserialize_from::<_, u64>(self, SizeLimit::Infinite));
        debug!("Deserializing message of len {}", len);
        deserialize_from(self, SizeLimit::Infinite).map_err(Error::from)
    }
}

impl<R: Read> Deserialize for R {}

trait Serialize: Write + Sized {
    fn serialize<T: serde::Serialize>(&mut self, value: &T) -> Result<()> {
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
    use std::net::TcpStream;
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
        let client: Client<(), u64, TcpStream> = Client::new(serve_handle.dialer()).unwrap();
        drop(client);
        serve_handle.shutdown();
    }

    #[test]
    fn simple() {
        let _ = env_logger::init();
        let server = Arc::new(Server::new());
        let serve_handle = server.clone().spawn("localhost:0").unwrap();
        // The explicit type is required so that it doesn't deserialize a u32 instead of u64
        let client: Client<(), u64, _> = Client::new(serve_handle.dialer()).unwrap();
        assert_eq!(0, client.rpc(()).unwrap());
        assert_eq!(1, server.count());
        assert_eq!(1, client.rpc(()).unwrap());
        assert_eq!(2, server.count());
        drop(client);
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
        let client: Client<(), u64, _> = Client::new(serve_handle.dialer()).unwrap();
        let thread = thread::spawn(move || serve_handle.shutdown());
        info!("force_shutdown:: rpc1: {:?}", client.rpc(()));
        thread.join().unwrap();
    }

    #[test]
    fn client_failed_rpc() {
        let _ = env_logger::init();
        let server = Arc::new(Server::new());
        let serve_handle = server.spawn_with_config("localhost:0",
                                                    Config { timeout: test_timeout() })
                                 .unwrap();
        let client: Arc<Client<(), u64, _>> = Arc::new(Client::new(serve_handle.dialer()).unwrap());
        client.rpc(()).unwrap();
        serve_handle.shutdown();
        match client.rpc(()) {
            Err(super::Error::ConnectionBroken) => {} // success
            otherwise => panic!("Expected Err(ConnectionBroken), got {:?}", otherwise),
        }
        let _ = client.rpc(()); // Test whether second failure hangs
    }

    #[test]
    fn concurrent() {
        let _ = env_logger::init();
        let concurrency = 10;
        let pool = Pool::new(concurrency);
        let server = Arc::new(BarrierServer::new(concurrency));
        let serve_handle = server.clone().spawn("localhost:0").unwrap();
        let client: Client<(), u64, _> = Client::new(serve_handle.dialer()).unwrap();
        pool.scoped(|scope| {
            for _ in 0..concurrency {
                let client = client.try_clone().unwrap();
                scope.execute(move || {
                    client.rpc(()).unwrap();
                });
            }
        });
        assert_eq!(concurrency as u64, server.count());
        drop(client);
        serve_handle.shutdown();
    }

    #[test]
    fn async() {
        let _ = env_logger::init();
        let server = Arc::new(Server::new());
        let serve_handle = server.spawn("localhost:0").unwrap();
        let client: Client<(), u64, _> = Client::new(serve_handle.dialer()).unwrap();

        // Drop future immediately; does the reader channel panic when sending?
        client.rpc_async(());
        // If the reader panicked, this won't succeed
        client.rpc_async(());

        drop(client);
        serve_handle.shutdown();
    }
}
