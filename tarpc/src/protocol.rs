use bincode;
use serde;
use std::fmt;
use std::io::{self, Read};
use std::convert;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::net::{TcpListener, TcpStream, SocketAddr, ToSocketAddrs};
use std::sync::{self, Arc, Condvar, Mutex};
use std::sync::mpsc::{channel, Sender, TryRecvError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::thread::{self, JoinHandle};

/// Client errors that can occur during rpc calls
#[derive(Debug)]
pub enum Error {
    /// An IO-related error
    Io(io::Error),
    /// An error in serialization
    Serialize(bincode::serde::SerializeError),
    /// An error in deserialization
    Deserialize(bincode::serde::DeserializeError),
    /// An internal message failed to send.
    /// Channels are used for the client's inter-thread communication. This message is
    /// propagated if the receiver unexpectedly hangs up.
    Sender,
    /// An internal message failed to be received.
    /// Channels are used for the client's inter-thread communication. This message is
    /// propagated if the sender unexpectedly hangs up.
    Receiver,
    /// The server hung up.
    ConnectionBroken,
}

impl convert::From<bincode::serde::SerializeError> for Error {
    fn from(err: bincode::serde::SerializeError) -> Error {
        match err {
            bincode::serde::SerializeError::IoError(err) => Error::Io(err),
            err => Error::Serialize(err),
        }
    }
}

impl convert::From<bincode::serde::DeserializeError> for Error {
    fn from(err: bincode::serde::DeserializeError) -> Error {
        match err {
            bincode::serde::DeserializeError::IoError(err) => Error::Io(err),
            err => Error::Deserialize(err),
        }
    }
}

impl convert::From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(err)
    }
}

impl<T> convert::From<sync::mpsc::SendError<T>> for Error {
    fn from(_: sync::mpsc::SendError<T>) -> Error {
        Error::Sender
    }
}

impl convert::From<sync::mpsc::RecvError> for Error {
    fn from(_: sync::mpsc::RecvError) -> Error {
        Error::Receiver
    }
}

/// Return type of rpc calls: either the successful return value, or a client error.
pub type Result<T> = ::std::result::Result<T, Error>;

struct ConnectionHandler {
    shutdown: Arc<AtomicBool>,
    open_connections: Arc<(Mutex<u64>, Condvar)>,
    timeout: Option<Duration>,
}

impl Drop for ConnectionHandler {
    fn drop(&mut self) {
        let &(ref count, ref cvar) = &*self.open_connections;
        *count.lock().unwrap() -= 1;
        cvar.notify_one();
        trace!("ConnectionHandler: finished serving client.");
    }
}

impl ConnectionHandler {
    fn handle_conn<F, Request, Reply>(&self, stream: TcpStream, f: F) -> Result<()>
        where Request: 'static + fmt::Debug + Send + serde::de::Deserialize + serde::ser::Serialize,
              Reply: 'static + fmt::Debug + serde::ser::Serialize,
              F: 'static + Clone + Serve<Request, Reply>
    {
        trace!("ConnectionHandler: serving client...");
        let mut read_stream = try!(stream.try_clone());
        let stream = Arc::new(Mutex::new(stream));
        loop {
            try!(read_stream.set_read_timeout(self.timeout));
            match bincode::serde::deserialize_from(&mut read_stream, bincode::SizeLimit::Infinite) {
                Ok(request_packet @ Packet::Shutdown) => {
                    let stream = stream.clone();
                    let mut my_stream = stream.lock().unwrap();
                    try!(bincode::serde::serialize_into(&mut *my_stream,
                                                        &request_packet,
                                                        bincode::SizeLimit::Infinite));
                    break;
                }
                Ok(Packet::Message(id, message)) => {
                    let f = f.clone();
                    let arc_stream = stream.clone();
                    thread::spawn(move || {
                        let reply = f.serve(message);
                        let reply_packet = Packet::Message(id, reply);
                        let mut my_stream = arc_stream.lock().unwrap();
                        bincode::serde::serialize_into(&mut *my_stream,
                                                       &reply_packet,
                                                       bincode::SizeLimit::Infinite)
                                       .unwrap();
                    });
                }
                Err(bincode::serde::DeserializeError::IoError(ref err))
                    if Self::timed_out(err.kind()) =>
                {
                    if !self.shutdown() {
                        warn!("ConnectionHandler: read timed out ({:?}). Server not shutdown, so retrying read.", err);
                        continue;
                    } else {
                        warn!("ConnectionHandler: read timed out ({:?}). Server shutdown, so closing connection.", err);
                        let mut stream = stream.lock().unwrap();
                        try!(bincode::serde::serialize_into(&mut *stream,
                                                            &Packet::Shutdown::<Reply>,
                                                            bincode::SizeLimit::Infinite));
                        break;
                    }
                }
                Err(e) => {
                    warn!("ConnectionHandler: closing client connection due to error while serving: {:?}", e);
                    return Err(e.into());
                }
            }
        }
        Ok(())
    }

    fn shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
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
pub fn serve_async<A, F, Request, Reply>(addr: A, f: F, read_timeout: Option<Duration>) -> io::Result<ServeHandle>
    where A: ToSocketAddrs,
          Request: 'static + fmt::Debug + Send + serde::de::Deserialize + serde::ser::Serialize,
          Reply: 'static + fmt::Debug + serde::ser::Serialize,
          F: 'static + Clone + Send + Serve<Request, Reply>
{
    let listener = try!(TcpListener::bind(&addr));
    let addr = try!(listener.local_addr());
    info!("serve_async: spinning up server on {:?}", addr);
    let (die_tx, die_rx) = channel();
    let join_handle = thread::spawn(move || {
        let shutdown = Arc::new(AtomicBool::new(false));
        let open_connections = Arc::new((Mutex::new(0), Condvar::new()));
        for conn in listener.incoming() {
            match die_rx.try_recv() {
                Ok(_) => {
                    info!("serve_async: shutdown received. Waiting for open connections to return...");
                    shutdown.store(true, Ordering::SeqCst);
                    let &(ref count, ref cvar) = &*open_connections;
                    let mut count = count.lock().unwrap();
                    while *count != 0 {
                        count = cvar.wait(count).unwrap();
                    }
                    info!("serve_async: shutdown complete ({} connections alive)", *count);
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
            let f = f.clone();
            let shutdown = shutdown.clone();
            let &(ref count, _) = &*open_connections;
            *count.lock().unwrap() += 1;
            let open_connections = open_connections.clone();
            thread::spawn(move || {
                let handler = ConnectionHandler {
                    shutdown: shutdown,
                    open_connections: open_connections,
                    timeout: read_timeout,
                };
                if let Err(err) = handler.handle_conn(conn, f) {
                    error!("ConnectionHandler: error in connection handling: {:?}", err);
                }
            });
        }
    });
    Ok(ServeHandle {
        tx: die_tx,
        join_handle: join_handle,
        addr: addr.clone(),
    })
}

/// A service provided by a server
pub trait Serve<Request, Reply>: Send + Sync {
    /// Return a reply for a given request
    fn serve(&self, request: Request) -> Reply;
}

impl<Request, Reply, S> Serve<Request, Reply> for Arc<S>
    where S: Serve<Request, Reply>
{
    fn serve(&self, request: Request) -> Reply {
        S::serve(self, request)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Packet<T> {
    Message(u64, T),
    Shutdown,
}

fn reader<Reply>(mut stream: TcpStream, requests: Arc<Mutex<HashMap<u64, Sender<Reply>>>>)
    where Reply: serde::Deserialize
{
    loop {
        let packet: bincode::serde::DeserializeResult<Packet<Reply>> =
            bincode::serde::deserialize_from(&mut stream, bincode::SizeLimit::Infinite);
        match packet {
            Ok(Packet::Message(id, reply)) => {
                debug!("Client: received message, id={}", id);
                let mut requests = requests.lock().unwrap();
                let reply_tx = requests.remove(&id).unwrap();
                reply_tx.send(reply).unwrap();
            }
            Ok(Packet::Shutdown) => {
                info!("Client: got shutdown message.");
                requests.lock().unwrap().clear();
                break;
            }
            // TODO: This shutdown logic is janky.. What's the right way to do this?
            Err(err) => panic!("unexpected error while parsing!: {:?}", err),
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
    requests: Arc<Mutex<HashMap<u64, Sender<Reply>>>>,
    reader_guard: Option<thread::JoinHandle<()>>,
    timeout: Option<Duration>,
    _request: PhantomData<Request>,
}

impl<Request, Reply> Client<Request, Reply>
    where Reply: serde::de::Deserialize + Send + 'static,
          Request: serde::ser::Serialize
{
    /// Create a new client that connects to `addr`
    pub fn new<A: ToSocketAddrs>(addr: A, timeout: Option<Duration>) -> io::Result<Self> {
        let stream = try!(TcpStream::connect(addr));
        let requests = Arc::new(Mutex::new(HashMap::new()));
        let reader_stream = try!(stream.try_clone());
        let reader_requests = requests.clone();
        let reader_guard = thread::spawn(move || reader(reader_stream, reader_requests));
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
        {
            let mut requests = self.requests.lock().unwrap();
            requests.insert(id, tx);
        }
        let packet = Packet::Message(id, request);
        try!(state.stream.set_write_timeout(self.timeout));
        try!(state.stream.set_read_timeout(self.timeout));
        debug!("Client: calling rpc({:?})", request);
        if let Err(err) = bincode::serde::serialize_into(&mut state.stream,
                                                         &packet,
                                                         bincode::SizeLimit::Infinite) {
            warn!("Client: failed to write packet.\nPacket: {:?}\nError: {:?}",
                  packet,
                  err);
            self.requests.lock().unwrap().remove(&id);
            return Err(err.into());
        }
        drop(state);
        Ok(try!(rx.recv()))
    }
}

impl<Request, Reply> Drop for Client<Request, Reply>
    where Request: serde::ser::Serialize
{
    fn drop(&mut self) {
        {
            let mut state = self.synced_state.lock().unwrap();
            let packet: Packet<Request> = Packet::Shutdown;
            if let Err(err) = bincode::serde::serialize_into(&mut state.stream,
                                                             &packet,
                                                             bincode::SizeLimit::Infinite) {
                warn!("While disconnecting client from server: {:?}", err);
            }
        }
        self.reader_guard.take().unwrap().join().unwrap();
    }
}

#[cfg(test)]
mod test {
    extern crate env_logger;

    use super::*;
    use std::sync::{Arc, Mutex, Barrier};
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

    impl Serve<Request, Reply> for Server {
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
        let client: Client<Request, Reply> = Client::new(serve_handle.local_addr().clone(),
                                                         test_timeout())
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
        let client = Client::new(addr, test_timeout()).unwrap();
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

    impl Serve<Request, Reply> for BarrierServer {
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
        let client: Arc<Client<Request, Reply>> = Arc::new(Client::new(addr, test_timeout())
                                                           .unwrap());
        let thread = thread::spawn(move || serve_handle.shutdown());
        info!("force_shutdown::client: {:?}", client.rpc(&Request::Increment));
        thread.join().unwrap();
    }

    #[test]
    fn concurrent() {
        let _ = env_logger::init();
        let server = Arc::new(BarrierServer::new(10));
        let serve_handle = serve_async("0.0.0.0:0", server.clone(), test_timeout()).unwrap();
        let addr = serve_handle.local_addr().clone();
        let client: Arc<Client<Request, Reply>> = Arc::new(Client::new(addr, test_timeout()).unwrap());
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
