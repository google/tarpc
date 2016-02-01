// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use bincode;
use serde;
use scoped_pool::Pool;
use std::fmt;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::convert;
use std::collections::HashMap;
use std::mem;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::thread::{self, JoinHandle};

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
            bincode::serde::DeserializeError::IoError(err) => Error::Io(Arc::new(err)),
            bincode::serde::DeserializeError::EndOfStreamError => Error::ConnectionBroken,
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

/// An asynchronous RPC call
pub struct Future<T> {
    rx: Receiver<Result<T>>,
    requests: Arc<Mutex<RpcFutures<T>>>
}

impl<T> Future<T> {
    /// Block until the result of the RPC call is available
    pub fn get(self) -> Result<T> {
        let requests = self.requests;
        self.rx.recv()
            .map_err(|_| requests.lock().unwrap().get_error())
            .and_then(|reply| reply)
    }
}

struct InflightRpcs {
    count: Mutex<u64>,
    cvar: Condvar,
}

impl InflightRpcs {
    fn new() -> InflightRpcs {
        InflightRpcs {
            count: Mutex::new(0),
            cvar: Condvar::new(),
        }
    }

    fn wait_until_zero(&self) {
        let mut count = self.count.lock().unwrap();
        while *count != 0 {
            count = self.cvar.wait(count).unwrap();
        }
        info!("serve_async: shutdown complete ({} connections alive)",
              *count);
    }

    fn increment(&self) {
        *self.count.lock().unwrap() += 1;
    }

    fn decrement(&self) {
        *self.count.lock().unwrap() -= 1;
    }


    fn decrement_and_notify(&self) {
        *self.count.lock().unwrap() -= 1;
        self.cvar.notify_one();
    }

}

struct ConnectionHandler<'a, S>
    where S: Serve
{
    read_stream: BufReader<TcpStream>,
    write_stream: Mutex<BufWriter<TcpStream>>,
    shutdown: &'a AtomicBool,
    inflight_rpcs: &'a InflightRpcs,
    server: S,
    pool: &'a Pool,
}

impl<'a, S> Drop for ConnectionHandler<'a, S> where S: Serve {
    fn drop(&mut self) {
        trace!("ConnectionHandler: finished serving client.");
        self.inflight_rpcs.decrement_and_notify();
    }
}

impl<'a, S> ConnectionHandler<'a, S> where S: Serve {
    fn handle_conn(&mut self) -> Result<()> {
        let ConnectionHandler {
            ref mut read_stream,
            ref write_stream,
            shutdown,
            inflight_rpcs,
            ref server,
            pool,
        } = *self;
        trace!("ConnectionHandler: serving client...");
        pool.scoped(|scope| {
            loop {
                match bincode::serde::deserialize_from(read_stream, bincode::SizeLimit::Infinite) {
                    Ok(Packet { rpc_id, message, }) => {
                        debug!("ConnectionHandler: serving request, id: {}, message: {:?}", rpc_id, message);
                        inflight_rpcs.increment();
                        scope.execute(move || {
                            let reply = server.serve(message);
                            let reply_packet = Packet {
                                rpc_id: rpc_id,
                                message: reply
                            };
                            let mut write_stream = write_stream.lock().unwrap();
                            if let Err(e) =
                                   bincode::serde::serialize_into(&mut *write_stream,
                                                                  &reply_packet,
                                                                  bincode::SizeLimit::Infinite) {
                                warn!("ConnectionHandler: failed to write reply to Client: {:?}",
                                      e);
                            }
                            if let Err(e) = write_stream.flush() {
                                warn!("ConnectionHandler: failed to flush reply to Client: {:?}",
                                      e);
                            }
                            inflight_rpcs.decrement();
                        });
                        if shutdown.load(Ordering::SeqCst) {
                            info!("ConnectionHandler: server shutdown, so closing connection.");
                            break;
                        }
                    }
                    Err(bincode::serde::DeserializeError::IoError(ref err))
                        if Self::timed_out(err.kind()) => {
                        if !shutdown.load(Ordering::SeqCst) {
                            continue;
                        } else {
                            info!("ConnectionHandler: read timed out ({:?}). Server shutdown, so \
                                   closing connection.",
                                  err);
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("ConnectionHandler: closing client connection due to {:?}",
                              e);
                        return Err(e.into());
                    }
                }
            }
            Ok(())
        })
    }

    fn timed_out(error_kind: io::ErrorKind) -> bool {
        match error_kind {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => true,
            _ => false,
        }
    }
}

/// Provides methods for blocking until the server completes,
pub struct ServeHandle {
    tx: Sender<()>,
    join_handle: JoinHandle<()>,
    addr: SocketAddr,
}

impl ServeHandle {
    /// Block until the server completes
    pub fn wait(self) {
        self.join_handle.join().unwrap();
    }

    /// Returns the address the server is bound to
    pub fn local_addr(&self) -> &SocketAddr {
        &self.addr
    }

    /// Shutdown the server. Gracefully shuts down the serve thread but currently does not
    /// gracefully close open connections.
    pub fn shutdown(self) {
        info!("ServeHandle: attempting to shut down the server.");
        self.tx.send(()).unwrap();
        if let Ok(_) = TcpStream::connect(self.addr) {
            self.join_handle.join().unwrap();
        } else {
            warn!("ServeHandle: best effort shutdown of serve thread failed");
        }
    }
}

/// Start
pub fn serve_async<A, S>(addr: A,
                         server: S,
                         read_timeout: Option<Duration>)
                         -> io::Result<ServeHandle>
    where A: ToSocketAddrs,
          S: 'static + Serve
{
    let listener = try!(TcpListener::bind(&addr));
    let addr = try!(listener.local_addr());
    info!("serve_async: spinning up server on {:?}", addr);
    let (die_tx, die_rx) = channel();
    let join_handle = thread::spawn(move || {
        let pool = Pool::new(100); // TODO(tjk): make this configurable, and expire idle threads
        let shutdown = AtomicBool::new(false);
        let inflight_rpcs = InflightRpcs::new();
        pool.scoped(|scope| {
            for conn in listener.incoming() {
                match die_rx.try_recv() {
                    Ok(_) => {
                        info!("serve_async: shutdown received. Waiting for open connections to \
                               return...");
                        shutdown.store(true, Ordering::SeqCst);
                        inflight_rpcs.wait_until_zero();
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        info!("serve_async: sender disconnected.");
                        break;
                    }
                    _ => (),
                }
                let conn = match conn {
                    Err(err) => {
                        error!("serve_async: failed to accept connection: {:?}", err);
                        return;
                    }
                    Ok(c) => c,
                };
                if let Err(err) = conn.set_read_timeout(read_timeout) {
                    info!("Server: could not set read timeout: {:?}", err);
                    return;
                }
                inflight_rpcs.increment();
                scope.execute(|| {
                    let mut handler = ConnectionHandler {
                        read_stream: BufReader::new(conn.try_clone().unwrap()),
                        write_stream: Mutex::new(BufWriter::new(conn)),
                        shutdown: &shutdown,
                        inflight_rpcs: &inflight_rpcs,
                        server: &server,
                        pool: &pool,
                    };
                    if let Err(err) = handler.handle_conn() {
                        info!("ConnectionHandler: err in connection handling: {:?}", err);
                    }
                });
            }
        });
    });
    Ok(ServeHandle {
        tx: die_tx,
        join_handle: join_handle,
        addr: addr.clone(),
    })
}

/// A service provided by a server
pub trait Serve: Send + Sync {
    /// The type of request received by the server
    type Request: 'static + fmt::Debug + serde::ser::Serialize + serde::de::Deserialize + Send;
    /// The type of reply sent by the server
    type Reply:  'static + fmt::Debug + serde::ser::Serialize + serde::de::Deserialize;

    /// Return a reply for a given request
    fn serve(&self, request: Self::Request) -> Self::Reply;
}

impl<P, S> Serve for P
    where P: Send + Sync + ::std::ops::Deref<Target=S>,
          S: Serve
{
    type Request = S::Request;
    type Reply = S::Reply;

    fn serve(&self, request: S::Request) -> S::Reply {
        S::serve(self, request)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Packet<T> {
    rpc_id: u64,
    message: T,
}

struct RpcFutures<Reply>(Result<HashMap<u64, Sender<Result<Reply>>>>);

impl<Reply> RpcFutures<Reply> {
    fn new() -> RpcFutures<Reply> {
        RpcFutures(Ok(HashMap::new()))
    }

    fn insert_tx(&mut self, id: u64, tx: Sender<Result<Reply>>) -> Result<()> {
        match self.0 {
            Ok(ref mut requests) => {
                requests.insert(id, tx);
                Ok(())
            }
            Err(ref e) => Err(e.clone()),
        }
    }

    fn remove_tx(&mut self, id: u64) -> Result<()> {
        match self.0 {
            Ok(ref mut requests) => {
                requests.remove(&id);
                Ok(())
            }
            Err(ref e) => Err(e.clone()),
        }
    }

    fn complete_reply(&mut self, id: u64, reply: Reply) {
        if let Some(tx) = self.0.as_mut().unwrap().remove(&id) {
            if let Err(e) = tx.send(Ok(reply)) {
                info!("Reader: could not complete reply: {:?}", e);
            }
        } else {
            warn!("RpcFutures: expected sender for id {} but got None!", id);
        }
    }

    fn set_error(&mut self, err: bincode::serde::DeserializeError) {
        let _ = mem::replace(&mut self.0, Err(err.into()));
    }

    fn get_error(&self) -> Error {
        self.0.as_ref().err().unwrap().clone()
    }
}

fn write<Request, Reply>(outbound: Receiver<(Request, Sender<Result<Reply>>)>,
                  requests: Arc<Mutex<RpcFutures<Reply>>>,
                  stream: TcpStream)
    where Request: serde::Serialize,
          Reply: serde::Deserialize,
{
    let mut next_id = 0;
    let mut stream = BufWriter::new(stream);
    loop {
        let (request, tx) = match outbound.recv() {
            Err(e) => {
                debug!("Writer: all senders have exited ({:?}). Returning.", e);
                return;
            }
            Ok(request) => request,
        };
        if let Err(e) = requests.lock().unwrap().insert_tx(next_id, tx.clone()) {
            report_error(&tx, e);
            // Once insert_tx returns Err, it will continue to do so. However, continue here so
            // that any other clients who sent requests will also recv the Err.
            continue;
        }
        let id = next_id;
        next_id += 1;
        let packet = Packet {
            rpc_id: id,
            message: request,
        };
        debug!("Writer: calling rpc({:?})", id);
        if let Err(e) = bincode::serde::serialize_into(&mut stream,
                                                         &packet,
                                                         bincode::SizeLimit::Infinite) {
            report_error(&tx, e.into());
            // Typically we'd want to notify the client of any Err returned by remove_tx, but in
            // this case the client already hit an Err, and doesn't need to know about this one, as
            // well.
            let _ = requests.lock().unwrap().remove_tx(id);
            continue;
        }
        if let Err(e) = stream.flush() {
            report_error(&tx, e.into());
        }
    }

    fn report_error<Reply>(tx: &Sender<Result<Reply>>, e: Error)
        where Reply: serde::Deserialize
    {
        // Clone the err so we can log it if sending fails
        if let Err(e2) = tx.send(Err(e.clone())) {
            debug!("Error encountered while trying to send an error. \
                   Initial error: {:?}; Send error: {:?}",
                   e,
                   e2);
        }
    }

}

fn read<Reply>(requests: Arc<Mutex<RpcFutures<Reply>>>, stream: TcpStream)
    where Reply: serde::Deserialize
{
    let mut stream = BufReader::new(stream);
    loop {
        let packet: bincode::serde::DeserializeResult<Packet<Reply>> =
            bincode::serde::deserialize_from(&mut stream, bincode::SizeLimit::Infinite);
        match packet {
            Ok(Packet {
                rpc_id: id,
                message: reply
            }) => {
                debug!("Client: received message, id={}", id);
                requests.lock().unwrap().complete_reply(id, reply);
            }
            Err(err) => {
                warn!("Client: reader thread encountered an unexpected error while parsing; \
                       returning now. Error: {:?}",
                      err);
                requests.lock().unwrap().set_error(err);
                break;
            }
        }
    }
}

/// A client stub that connects to a server to run rpcs.
pub struct Client<Request, Reply>
    where Request: serde::ser::Serialize
{
    // The guard is in an option so it can be joined in the drop fn
    reader_guard: Arc<Option<thread::JoinHandle<()>>>,
    outbound: Sender<(Request, Sender<Result<Reply>>)>,
    requests: Arc<Mutex<RpcFutures<Reply>>>,
    shutdown: TcpStream,
}

impl<Request, Reply> Client<Request, Reply>
    where Request: serde::ser::Serialize + Send + 'static,
          Reply: serde::de::Deserialize + Send + 'static
{
    /// Create a new client that connects to `addr`. The client uses the given timeout
    /// for both reads and writes.
    pub fn new<A: ToSocketAddrs>(addr: A, timeout: Option<Duration>) -> io::Result<Self> {
        let stream = try!(TcpStream::connect(addr));
        try!(stream.set_read_timeout(timeout));
        try!(stream.set_write_timeout(timeout));
        let reader_stream = try!(stream.try_clone());
        let writer_stream = try!(stream.try_clone());
        let requests = Arc::new(Mutex::new(RpcFutures::new()));
        let reader_requests = requests.clone();
        let writer_requests = requests.clone();
        let (tx, rx) = channel();
        let reader_guard = thread::spawn(move || read(reader_requests, reader_stream));
        thread::spawn(move || write(rx, writer_requests, writer_stream));
        Ok(Client {
            reader_guard: Arc::new(Some(reader_guard)),
            outbound: tx,
            requests: requests,
            shutdown: stream,
        })
    }

    /// Clones the Client so that it can be shared across threads.
    pub fn try_clone(&self) -> io::Result<Client<Request, Reply>> {
        Ok(Client {
            reader_guard: self.reader_guard.clone(),
            outbound: self.outbound.clone(),
            requests: self.requests.clone(),
            shutdown: try!(self.shutdown.try_clone()),
        })
    }

    fn rpc_internal(&self, request: Request) -> Receiver<Result<Reply>>
        where Request: serde::ser::Serialize + fmt::Debug + Send + 'static
    {
        let (tx, rx) = channel();
        self.outbound.send((request, tx)).unwrap();
        rx
    }

    /// Run the specified rpc method on the server this client is connected to
    pub fn rpc(&self, request: Request) -> Result<Reply>
        where Request: serde::ser::Serialize + fmt::Debug + Send + 'static
    {
        self.rpc_internal(request)
            .recv()
            .map_err(|_| self.requests.lock().unwrap().get_error())
            .and_then(|reply| reply)
    }

    /// Asynchronously run the specified rpc method on the server this client is connected to
    pub fn rpc_async(&self, request: Request) -> Future<Reply>
        where Request: serde::ser::Serialize + fmt::Debug + Send + 'static
    {
        Future {
            rx: self.rpc_internal(request),
            requests: self.requests.clone(),
        }
    }
}

impl<Request, Reply> Drop for Client<Request, Reply>
    where Request: serde::ser::Serialize
{
    fn drop(&mut self) {
        debug!("Dropping Client.");
        if let Some(reader_guard) = Arc::get_mut(&mut self.reader_guard) {
            debug!("Attempting to shut down writer and reader threads.");
            if let Err(e) = self.shutdown.shutdown(::std::net::Shutdown::Both) {
                warn!("Client: couldn't shutdown writer and reader threads: {:?}", e);
            } else {
                // We only join if we know the TcpStream was shut down. Otherwise we might never
                // finish.
                debug!("Joining writer and reader.");
                reader_guard.take().unwrap().join().unwrap();
                debug!("Successfully joined writer and reader.");
            }
        }
    }
}

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
