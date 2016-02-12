// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::{self, SizeLimit};
use bincode::serde::{deserialize_from, serialize_into};
use serde;
use std::io::{self, Read, Write};
use std::convert;
use std::sync::Arc;

mod client;
mod server;

pub use self::client::{Client, Future};
pub use self::server::{Serve, ServeHandle, serve_async};

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
            bincode::serde::DeserializeError::IoError(ref err)
                if err.kind() == io::ErrorKind::ConnectionReset => Error::ConnectionBroken,
            bincode::serde::DeserializeError::EndOfStreamError => Error::ConnectionBroken,
            bincode::serde::DeserializeError::IoError(err) => Error::Io(Arc::new(err)),
            err => panic!("Unexpected error during deserialization: {:?}", err),
        }
    }
}

impl convert::From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(Arc::new(err))
    }
}

/// Return type of rpc calls: either the successful return value, or a client error.
pub type Result<T> = ::std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Packet<T> {
    rpc_id: u64,
    message: T,
}

trait Deserialize: Read + Sized {
    fn deserialize<T: serde::Deserialize>(&mut self) -> Result<T> {
        deserialize_from(self, SizeLimit::Infinite)
            .map_err(Error::from)
    }
}

impl<R: Read> Deserialize for R {}

trait Serialize: Write + Sized {
    fn serialize<T: serde::Serialize>(&mut self, value: &T) -> Result<()> {
        try!(serialize_into(self, value, SizeLimit::Infinite));
        try!(self.flush());
        Ok(())
    }
}

impl<W: Write> Serialize for W {}

#[cfg(test)]
mod test {
    extern crate env_logger;
    use super::{Client, Serve, serve_async};
    use scoped_pool::Pool;
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use std::time::Duration;

    fn test_timeout() -> Option<Duration> {
        Some(Duration::from_secs(1))
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
    enum Request {
        Increment,
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum Reply {
        Increment(u64),
    }

    struct Server {
        counter: Mutex<u64>,
    }

    impl Serve for Server {
        type Request = Request;
        type Reply = Reply;

        fn serve(&self, _: Request) -> Reply {
            let mut counter = self.counter.lock().unwrap();
            let reply = Reply::Increment(*counter);
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
        let serve_handle = serve_async("localhost:0", server.clone(), test_timeout()).unwrap();
        let client: Client<Request, Reply> = Client::new(serve_handle.local_addr(), None).unwrap();
        drop(client);
        serve_handle.shutdown();
    }

    #[test]
    fn simple() {
        let _ = env_logger::init();
        let server = Arc::new(Server::new());
        let serve_handle = serve_async("localhost:0", server.clone(), test_timeout()).unwrap();
        let addr = serve_handle.local_addr().clone();
        let client = Client::new(addr, None).unwrap();
        assert_eq!(Reply::Increment(0),
                   client.rpc(Request::Increment).unwrap());
        assert_eq!(1, server.count());
        assert_eq!(Reply::Increment(1),
                   client.rpc(Request::Increment).unwrap());
        assert_eq!(2, server.count());
        drop(client);
        serve_handle.shutdown();
    }

    struct BarrierServer {
        barrier: Barrier,
        inner: Server,
    }

    impl Serve for BarrierServer {
        type Request = Request;
        type Reply = Reply;
        fn serve(&self, request: Request) -> Reply {
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
        let serve_handle = serve_async("localhost:0", server, Some(Duration::new(0, 10))).unwrap();
        let addr = serve_handle.local_addr().clone();
        let client: Client<Request, Reply> = Client::new(addr, None).unwrap();
        let thread = thread::spawn(move || serve_handle.shutdown());
        info!("force_shutdown:: rpc1: {:?}", client.rpc(Request::Increment));
        thread.join().unwrap();
    }

    #[test]
    fn client_failed_rpc() {
        let _ = env_logger::init();
        let server = Arc::new(Server::new());
        let serve_handle = serve_async("localhost:0", server, test_timeout()).unwrap();
        let addr = serve_handle.local_addr().clone();
        let client: Arc<Client<Request, Reply>> = Arc::new(Client::new(addr, None).unwrap());
        client.rpc(Request::Increment).unwrap();
        serve_handle.shutdown();
        match client.rpc(Request::Increment) {
            Err(super::Error::ConnectionBroken) => {} // success
            otherwise => panic!("Expected Err(ConnectionBroken), got {:?}", otherwise),
        }
        let _ = client.rpc(Request::Increment); // Test whether second failure hangs
    }

    #[test]
    fn concurrent() {
        let _ = env_logger::init();
        let concurrency = 10;
        let pool = Pool::new(concurrency);
        let server = Arc::new(BarrierServer::new(concurrency));
        let serve_handle = serve_async("localhost:0", server.clone(), test_timeout()).unwrap();
        let addr = serve_handle.local_addr().clone();
        let client: Client<Request, Reply> = Client::new(addr, None).unwrap();
        pool.scoped(|scope| {
            for _ in 0..concurrency {
                let client = client.try_clone().unwrap();
                scope.execute(move || { client.rpc(Request::Increment).unwrap(); });
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
        let serve_handle = serve_async("localhost:0", server.clone(), None).unwrap();
        let addr = serve_handle.local_addr().clone();
        let client: Client<Request, Reply> = Client::new(addr, None).unwrap();

        // Drop future immediately; does the reader channel panic when sending?
        client.rpc_async(Request::Increment);
        // If the reader panicked, this won't succeed
        client.rpc_async(Request::Increment);

        drop(client);
        serve_handle.shutdown();
    }
}
