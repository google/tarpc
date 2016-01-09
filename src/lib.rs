#![feature(const_fn)]
#![feature(custom_derive, plugin)]
#![plugin(serde_macros)]

extern crate serde;
extern crate serde_json;

use serde::Deserialize;
use std::fmt;
use std::io::{self, Read};
use std::convert;
use std::collections::HashMap;
use std::net::{
    TcpListener,
    TcpStream,
    SocketAddr,
    ToSocketAddrs,
};
use std::sync::{
    self,
    Mutex,
    Arc,
};
use std::sync::mpsc::{
    channel,
    Sender,
    TryRecvError,
};
use std::thread::{
    self,
    JoinHandle,
};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Json(serde_json::Error),
    Sender,
    Unimplemented,
}

impl convert::From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Error {
        match err {
            serde_json::Error::IoError(err) => Error::Io(err),
            err => Error::Json(err),
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

pub type Result<T> = std::result::Result<T, Error>;

fn handle_conn<F, Request, Reply>(stream: TcpStream, f: F) -> Result<()>
    where Request: 'static + fmt::Debug + Send + serde::de::Deserialize + serde::ser::Serialize,
          Reply: 'static + fmt::Debug + serde::ser::Serialize,
          F: 'static + Clone + Serve<Request, Reply>
{
    let read_stream = try!(stream.try_clone());
    let mut de = serde_json::Deserializer::new(read_stream.bytes());
    let stream = Arc::new(Mutex::new(stream));
    loop {
        let request_packet: Packet<Request> = try!(Packet::deserialize(&mut de));
        match request_packet {
            Packet::Shutdown => {
                let stream = stream.clone();
                let mut my_stream = stream.lock().unwrap();
                try!(serde_json::to_writer(&mut *my_stream, &request_packet));
                break;
            },
            Packet::Message(id, message) => {
                let f = f.clone();
                let arc_stream = stream.clone();
                thread::spawn(move || {
                    let reply = f.serve(message);
                    let reply_packet = Packet::Message(id, reply);
                    let mut my_stream = arc_stream.lock().unwrap();
                    serde_json::to_writer(&mut *my_stream, &reply_packet).unwrap();
                });
            },
        }
    }
    Ok(())
}


pub struct Shutdown {
    tx: Sender<()>,
    join_handle: JoinHandle<()>,
    addr: SocketAddr,
}


impl Shutdown {
    pub fn wait(self) {
        self.join_handle.join().unwrap();
    }

    pub fn shutdown(self) {
        self.tx.send(()).expect(&line!().to_string());
        TcpStream::connect(&self.addr).unwrap();
        self.join_handle.join().expect(&line!().to_string());
    }
}

pub fn serve_async<A, F, Request, Reply>(addr: A, f: F) -> io::Result<Shutdown>
    where A: ToSocketAddrs,
          Request: 'static + fmt::Debug + Send + serde::de::Deserialize + fmt::Debug + serde::ser::Serialize,
          Reply: 'static + fmt::Debug + serde::ser::Serialize,
          F: 'static + Clone + Serve<Request, Reply>,
{
    let addr = addr.to_socket_addrs().unwrap().next().unwrap();
    let listener = try!(TcpListener::bind(addr.clone()));
    let (die_tx, die_rx) = channel();
    let join_handle = thread::spawn(move || {
        for conn in listener.incoming() {
            match die_rx.try_recv() {
                Ok(_) => break,
                Err(TryRecvError::Disconnected) => {
                    println!("serve: sender disconnected ");
                    break;
                },
                _ => (),
            }
            let conn = match conn {
                Err(err) => {
                    println!("I couldn't unwrap the connection :( {:?}", err);
                    return;
                },
                Ok(c) => c,
            };
            let f = f.clone();
            thread::spawn(move || {
                if let Err(err) = handle_conn(conn, f) {
                    println!("error handling connection: {:?}", err);
                }
            });
        }
    });
    Ok(Shutdown{
        tx: die_tx,
        join_handle: join_handle,
        addr: addr.clone(),
    })
}

pub trait Serve<Request, Reply>: Send + Sync {
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

fn reader<Reply>(
    stream: TcpStream,
    requests: Arc<Mutex<HashMap<u64, Sender<Reply>>>>)
    where Reply: serde::Deserialize
{
    let mut de = serde_json::Deserializer::new(stream.bytes());
    loop {
        match Packet::deserialize(&mut de) {
            Ok(Packet::Message(id, reply)) => {
                let mut requests = requests.lock().unwrap();
                let reply_tx = requests.remove(&id).unwrap();
                reply_tx.send(reply).unwrap();
            },
            Ok(Packet::Shutdown) => {
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

pub struct Client<Request, Reply>
    where Request: serde::ser::Serialize
{
    synced_state: Mutex<SyncedClientState>,
    requests: Arc<Mutex<HashMap<u64, Sender<Reply>>>>,
    reader_guard: Option<thread::JoinHandle<()>>,
    _request: std::marker::PhantomData<Request>,
}

impl<Request, Reply> Client<Request, Reply>
    where Reply: serde::de::Deserialize + Send + 'static,
          Request: serde::ser::Serialize
{
    pub fn new(stream: TcpStream) -> io::Result<Self> {
        let requests = Arc::new(Mutex::new(HashMap::new()));
        let reader_stream = try!(stream.try_clone());
        let reader_requests = requests.clone();
        let reader_guard =
            thread::spawn(move || reader(reader_stream, reader_requests));
        Ok(Client{
            synced_state: Mutex::new(SyncedClientState{
                next_id: 0,
                stream: stream,
            }),
            requests: requests,
            reader_guard: Some(reader_guard),
            _request: std::marker::PhantomData,
        })
    }

    pub fn rpc(&self, request: &Request) -> Result<Reply>
        where Request: serde::ser::Serialize + Send + 'static
    {
        let (tx, rx) = channel();
        let mut state = self.synced_state.lock().unwrap();
        let id = increment(&mut state.next_id);
        {
            let mut requests = self.requests.lock().unwrap();
            requests.insert(id, tx);
        }
        let packet = Packet::Message(id, request);
        try!(serde_json::to_writer(&mut state.stream, &packet));
        drop(state);
        Ok(rx.recv().unwrap())
    }
}

impl<Request, Reply> Drop for Client<Request, Reply>
    where Request: serde::ser::Serialize
{
    fn drop(&mut self) {
        {
            let mut state = self.synced_state.lock().unwrap();
            let packet: Packet<Request> = Packet::Shutdown;
            if let Err(err) = serde_json::to_writer(&mut state.stream, &packet) {
                println!("WARN: while disconnecting client from server: {:?}", err);
            }
        }
        self.reader_guard.take().unwrap().join().unwrap();
    }
}

#[cfg(test)]
mod test {
    use serde;
    use super::*;
    use std::fmt;
    use std::net::{TcpStream, SocketAddr, ToSocketAddrs};
    use std::sync::{Arc, Mutex, Barrier};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    const PORT: AtomicUsize = AtomicUsize::new(10000);

    fn next_addr() -> SocketAddr {
        let addr = format!("127.0.0.1:{}", PORT.fetch_add(1, Ordering::SeqCst));
        addr.to_socket_addrs().unwrap().next().unwrap()
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
    enum Request {
        Increment
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum Reply {
        Increment(u64)
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
            Server{counter: Mutex::new(0)}
        }

        fn count(&self) -> u64 {
            *self.counter.lock().unwrap()
        }
    }

    fn wtf<F, Request, Reply>(server: F) -> (SocketAddr, Shutdown)
        where Request: 'static + fmt::Debug + Send + serde::de::Deserialize + fmt::Debug + serde::ser::Serialize,
              Reply: 'static + fmt::Debug + Send + serde::ser::Serialize,
              F: 'static + Clone + Serve<Request, Reply>
    {
        let mut addr;
        let mut shutdown;
        while let &Err(_) = {shutdown = serve_async({addr = next_addr(); &addr}, server.clone()); &shutdown} { }
        (addr, shutdown.unwrap())
    }

    #[test]
    fn test_handle() {
        let server = Arc::new(Server::new());
        let (addr, shutdown) = wtf(server.clone());
        let client_stream = TcpStream::connect(&addr).unwrap();
        let client: Client<Request, Reply> = Client::new(client_stream).expect(&line!().to_string());
        drop(client);
        shutdown.shutdown();
    }

    #[test]
    fn test() {
        let server = Arc::new(Server::new());
        let (addr, shutdown) = wtf(server.clone());
        let client_stream = TcpStream::connect(&addr).unwrap();
        let client = Client::new(client_stream).unwrap();
        assert_eq!(Reply::Increment(0), client.rpc(&Request::Increment).unwrap());
        assert_eq!(1, server.count());
        assert_eq!(Reply::Increment(1), client.rpc(&Request::Increment).unwrap());
        assert_eq!(2, server.count());
        drop(client);
        shutdown.shutdown();
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
            BarrierServer{barrier: Barrier::new(n), inner: Server::new()}
        }

        fn count(&self) -> u64 {
            self.inner.count()
        }
    }

    #[test]
    fn test_concurrent() {
        let server = Arc::new(BarrierServer::new(10));
        let (addr, shutdown) = wtf(server.clone());
        let client_stream = TcpStream::connect(&addr).unwrap();
        let client: Arc<Client<Request, Reply>> = Arc::new(Client::new(client_stream).unwrap());
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
        shutdown.shutdown();
    }
}
