// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use bincode;
use serde;
use crossbeam;
use std::fmt;
use std::io::{self, Read};
use std::convert;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::mem;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex};
use std::sync::mpsc::{Sender, TryRecvError, channel};
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
    read_stream: TcpStream,
    write_stream: Mutex<TcpStream>,
    shutdown: &'a AtomicBool,
    inflight_rpcs: &'a InflightRpcs,
    timeout: Option<Duration>,
    server: S,
}

impl<'a, S> Drop for ConnectionHandler<'a, S> where S: Serve {
    fn drop(&mut self) {
        trace!("ConnectionHandler: finished serving client.");
        self.inflight_rpcs.decrement_and_notify();
    }
}

impl<'a, S> ConnectionHandler<'a, S> where S: Serve {
    fn read<Request>(read_stream: &mut TcpStream,
                     timeout: Option<Duration>)
                     -> bincode::serde::DeserializeResult<Packet<Request>>
        where Request: serde::de::Deserialize
    {
        try!(read_stream.set_read_timeout(timeout));
        bincode::serde::deserialize_from(read_stream, bincode::SizeLimit::Infinite)
    }

    fn handle_conn(&mut self) -> Result<()> {
        let ConnectionHandler {
            ref mut read_stream,
            ref write_stream,
            shutdown,
            inflight_rpcs,
            timeout,
            ref server,
        } = *self;
        trace!("ConnectionHandler: serving client...");
        crossbeam::scope(|scope| {
            loop {
                match Self::read(read_stream, timeout) {
                    Ok(Packet { rpc_id, message, }) => {
                        inflight_rpcs.increment();
                        scope.spawn(move || {
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
                            info!("ConnectionHandler: read timed out ({:?}). Server not \
                                   shutdown, so retrying read.",
                                  err);
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
        self.tx.send(()).expect(&line!().to_string());
        if let Ok(_) = TcpStream::connect(self.addr) {
            self.join_handle.join().expect(&line!().to_string());
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
        let shutdown = AtomicBool::new(false);
        let inflight_rpcs = InflightRpcs::new();
        crossbeam::scope(|scope| {
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
                inflight_rpcs.increment();
                scope.spawn(|| {
                    let mut handler = ConnectionHandler {
                        read_stream: conn.try_clone().unwrap(),
                        write_stream: Mutex::new(conn),
                        shutdown: &shutdown,
                        inflight_rpcs: &inflight_rpcs,
                        timeout: read_timeout,
                        server: &server,
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

struct RpcFutures<Reply>(Result<HashMap<u64, Sender<Reply>>>);

impl<Reply> RpcFutures<Reply> {
    fn new() -> RpcFutures<Reply> {
        RpcFutures(Ok(HashMap::new()))
    }

    fn insert_tx(&mut self, id: u64, tx: Sender<Reply>) -> Result<()> {
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
            tx.send(reply).unwrap();
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

struct Reader<Reply> {
    requests: Arc<Mutex<RpcFutures<Reply>>>,
}

impl<Reply> Reader<Reply> {
    fn read(self, mut stream: TcpStream)
        where Reply: serde::Deserialize
    {
        loop {
            let packet: bincode::serde::DeserializeResult<Packet<Reply>> =
                bincode::serde::deserialize_from(&mut stream, bincode::SizeLimit::Infinite);
            match packet {
                Ok(Packet {
                    rpc_id: id,
                    message: reply
                }) => {
                    debug!("Client: received message, id={}", id);
                    self.requests.lock().unwrap().complete_reply(id, reply);
                }
                Err(err) => {
                    warn!("Client: reader thread encountered an unexpected error while parsing; \
                           returning now. Error: {:?}",
                          err);
                    self.requests.lock().unwrap().set_error(err);
                    break;
                }
            }
        }
    }
}

fn increment(cur_id: &mut u64) -> u64 {
    let id = *cur_id;
    *cur_id += 1;
    id
}

struct SyncedClientState {
    next_id: u64,
    stream: TcpStream,
}

/// A client stub that connects to a server to run rpcs.
pub struct Client<Request, Reply>
    where Request: serde::ser::Serialize
{
    synced_state: Mutex<SyncedClientState>,
    requests: Arc<Mutex<RpcFutures<Reply>>>,
    reader_guard: Option<thread::JoinHandle<()>>,
    timeout: Option<Duration>,
    _request: PhantomData<Request>,
}

impl<Request, Reply> Client<Request, Reply>
    where Reply: serde::de::Deserialize + Send + 'static,
          Request: serde::ser::Serialize
{
    /// Create a new client that connects to `addr`. The client uses the given timeout
    /// for both reads and writes.
    pub fn new<A: ToSocketAddrs>(addr: A, timeout: Option<Duration>) -> io::Result<Self> {
        let stream = try!(TcpStream::connect(addr));
        let requests = Arc::new(Mutex::new(RpcFutures::new()));
        let reader_stream = try!(stream.try_clone());
        let reader = Reader { requests: requests.clone() };
        let reader_guard = thread::spawn(move || reader.read(reader_stream));
        Ok(Client {
            synced_state: Mutex::new(SyncedClientState {
                next_id: 0,
                stream: stream,
            }),
            requests: requests,
            reader_guard: Some(reader_guard),
            timeout: timeout,
            _request: PhantomData,
        })
    }

    /// Run the specified rpc method on the server this client is connected to
    pub fn rpc(&self, request: &Request) -> Result<Reply>
        where Request: serde::ser::Serialize + fmt::Debug + Send + 'static
    {
        let (tx, rx) = channel();
        let mut state = self.synced_state.lock().unwrap();
        let id = increment(&mut state.next_id);
        try!(self.requests.lock().unwrap().insert_tx(id, tx));
        let packet = Packet {
            rpc_id: id,
            message: request,
        };
        try!(state.stream.set_write_timeout(self.timeout));
        try!(state.stream.set_read_timeout(self.timeout));
        debug!("Client: calling rpc({:?})", request);
        if let Err(err) = bincode::serde::serialize_into(&mut state.stream,
                                                         &packet,
                                                         bincode::SizeLimit::Infinite) {
            warn!("Client: failed to write packet.\nPacket: {:?}\nError: {:?}",
                  packet,
                  err);
            try!(self.requests.lock().unwrap().remove_tx(id));
        }
        drop(state);
        match rx.recv() {
            Ok(msg) => Ok(msg),
            Err(_) => Err(self.requests.lock().unwrap().get_error()),
        }
    }
}

impl<Request, Reply> Drop for Client<Request, Reply>
    where Request: serde::ser::Serialize
{
    fn drop(&mut self) {
        if let Err(e) = self.synced_state
                            .lock()
                            .unwrap()
                            .stream
                            .shutdown(::std::net::Shutdown::Both) {
            warn!("Client: couldn't shutdown reader thread: {:?}", e);
        } else {
            self.reader_guard.take().unwrap().join().unwrap();
        }
    }
}

#[cfg(test)]
mod test {
    extern crate env_logger;

    use super::{Client, Serve, serve_async};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use std::time::Duration;

    fn test_timeout() -> Option<Duration> {
        Some(Duration::from_millis(1))
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
        let serve_handle = serve_async("0.0.0.0:0", server.clone(), test_timeout()).unwrap();
        let client: Client<Request, Reply> = Client::new(serve_handle.local_addr().clone(), None)
                                                 .expect(&line!().to_string());
        drop(client);
        serve_handle.shutdown();
    }

    #[test]
    fn simple() {
        let _ = env_logger::init();
        let server = Arc::new(Server::new());
        let serve_handle = serve_async("0.0.0.0:0", server.clone(), test_timeout()).unwrap();
        let addr = serve_handle.local_addr().clone();
        let client = Client::new(addr, None).unwrap();
        assert_eq!(Reply::Increment(0),
                   client.rpc(&Request::Increment).unwrap());
        assert_eq!(1, server.count());
        assert_eq!(Reply::Increment(1),
                   client.rpc(&Request::Increment).unwrap());
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
        let serve_handle = serve_async("0.0.0.0:0", server, Some(Duration::new(0, 10))).unwrap();
        let addr = serve_handle.local_addr().clone();
        let client: Arc<Client<Request, Reply>> = Arc::new(Client::new(addr, None).unwrap());
        let thread = thread::spawn(move || serve_handle.shutdown());
        info!("force_shutdown:: rpc1: {:?}",
              client.rpc(&Request::Increment));
        thread.join().unwrap();
    }

    #[test]
    fn client_failed_rpc() {
        let _ = env_logger::init();
        let server = Arc::new(Server::new());
        let serve_handle = serve_async("0.0.0.0:0", server, Some(Duration::new(0, 10))).unwrap();
        let addr = serve_handle.local_addr().clone();
        let client: Arc<Client<Request, Reply>> = Arc::new(Client::new(addr, None).unwrap());
        serve_handle.shutdown();
        match client.rpc(&Request::Increment) {
            Err(super::Error::ConnectionBroken) => {} // success
            otherwise => panic!("Expected Err(ConnectionBroken), got {:?}", otherwise),
        }
        let _ = client.rpc(&Request::Increment); // Test whether second failure hangs
    }

    #[test]
    fn concurrent() {
        let _ = env_logger::init();
        let server = Arc::new(BarrierServer::new(10));
        let serve_handle = serve_async("0.0.0.0:0", server.clone(), test_timeout()).unwrap();
        let addr = serve_handle.local_addr().clone();
        let client: Arc<Client<Request, Reply>> = Arc::new(Client::new(addr, None).unwrap());
        let mut join_handles = vec![];
        for _ in 0..10 {
            let my_client = client.clone();
            join_handles.push(thread::spawn(move || my_client.rpc(&Request::Increment).unwrap()));
        }
        for handle in join_handles.into_iter() {
            handle.join().unwrap();
        }
        assert_eq!(10, server.count());
        let client = match Arc::try_unwrap(client) {
            Err(_) => panic!("couldn't unwrap arc"),
            Ok(c) => c,
        };
        drop(client);
        serve_handle.shutdown();
    }
}
