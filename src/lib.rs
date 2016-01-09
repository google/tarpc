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
};
use std::sync::{
    self,
    Mutex,
    Arc,
};
use std::sync::mpsc::{
    channel,
    Sender,
};
use std::thread;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Json(serde_json::Error),
    Sender,
    Unimplemented,
    Impossible
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

pub fn handle_conn<F, Request, Reply>(mut stream: TcpStream, f: Arc<F>) -> Result<()>
    where Request: fmt::Debug + serde::de::Deserialize + serde::ser::Serialize,
          Reply: fmt::Debug + serde::ser::Serialize,
          F: Serve<Request, Reply>
{
    let read_stream = try!(stream.try_clone());
    let mut de = serde_json::Deserializer::new(read_stream.bytes());
    loop {
        println!("read");
        let request_packet: Packet<Request> = try!(Packet::deserialize(&mut de));
        match request_packet {
            Packet::Shutdown => {
                println!("server shutting down");
                try!(serde_json::to_writer(&mut stream, &request_packet));
                break;
            },
            Packet::Message(id, message) => {
                let reply = try!(f.serve(&message));
                let reply_packet = Packet::Message(id, reply);
                try!(serde_json::to_writer(&mut stream, &reply_packet));
            },
        }
    }
    Ok(())
}

pub fn serve<F, Request, Reply>(listener: TcpListener, f: Arc<F>) -> Error
    where Request: fmt::Debug + serde::de::Deserialize + fmt::Debug + serde::ser::Serialize,
          Reply: fmt::Debug + serde::ser::Serialize,
          F: 'static + Serve<Request, Reply>,
{
    for conn in listener.incoming() {
        let conn = match conn {
            Err(err) => return convert::From::from(err),
            Ok(c) => c,
        };
        let f = f.clone();
        thread::spawn(move || {
            if let Err(err) = handle_conn(conn, f) {
                println!("error handling connection: {:?}", err);
            }
        });
    }
    Error::Impossible
}

pub trait Serve<Request, Reply>: Send + Sync {
    fn serve(&self, request: &Request) -> io::Result<Reply>;
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
                println!("reader shutting down");
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

pub struct Client<Reply> {
    synced_state: Mutex<SyncedClientState>,
    requests: Arc<Mutex<HashMap<u64, Sender<Reply>>>>,
    reader_guard: thread::JoinHandle<()>,
}

impl<Reply> Client<Reply>
    where Reply: serde::de::Deserialize + Send + 'static
{
    pub fn new(stream: TcpStream) -> Result<Self> {
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
            reader_guard: reader_guard,
        })
    }

    pub fn rpc<Request>(&self, request: &Request) -> Result<Reply>
        where Request: serde::ser::Serialize + Clone + Send + 'static
    {
        let (tx, rx) = channel();
        let mut state = self.synced_state.lock().unwrap();
        let id = increment(&mut state.next_id);
        {
            let mut requests = self.requests.lock().unwrap();
            requests.insert(id, tx);
        }
        let packet = Packet::Message(id, request.clone());
        try!(serde_json::to_writer(&mut state.stream, &packet));
        Ok(rx.recv().unwrap())
    }

    pub fn join<Request: serde::Serialize>(self) -> Result<()> {
        {
            let mut state = self.synced_state.lock().unwrap();
            let packet: Packet<Request> = Packet::Shutdown;
            try!(serde_json::to_writer(&mut state.stream, &packet));
        }
        self.reader_guard.join().unwrap();
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::io;
    use std::net::{TcpStream, TcpListener, SocketAddr};
    use std::str::FromStr;
    use std::sync::{Arc, Mutex, Barrier};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    const port: AtomicUsize = AtomicUsize::new(10000);

    fn pair() -> (TcpStream, TcpListener) {
        let addr = format!("127.0.0.1:{}", port.fetch_add(1, Ordering::SeqCst));
        println!("what the fuck {}", &addr);
        // Do this one first so that we don't get connection refused :)
        let listener = TcpListener::bind(&*addr).unwrap();
        (TcpStream::connect(&*addr).unwrap(), listener)
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
        fn serve(&self, _: &Request) -> io::Result<Reply> {
            let mut counter = self.counter.lock().unwrap();
            let reply = Reply::Increment(*counter);
            *counter += 1;
            Ok(reply)
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

    #[test]
    fn test() {
        let (client_stream, server_streams) = pair();
        let server = Arc::new(Server::new());
        let thread_server = server.clone();
        let guard = thread::spawn(move || serve(server_streams, thread_server));
        let client = Client::new(client_stream).unwrap();
        assert_eq!(Reply::Increment(0), client.rpc(&Request::Increment).unwrap());
        assert_eq!(1, server.count());
        assert_eq!(Reply::Increment(1), client.rpc(&Request::Increment).unwrap());
        assert_eq!(2, server.count());
        println!("joining client");
        client.join::<Request>().unwrap();
        println!("joining server");
        guard.join();
    }

    /*
    struct BarrierServer {
        barrier: Barrier,
        inner: Server,
    }

    impl Serve<Request, Reply> for BarrierServer {
        fn serve(&self, request: &Request) -> io::Result<Reply> {
            self.barrier.wait();
            let reply = try!(self.inner.serve(request));
            Ok(reply)
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
        let (client_stream, server_streams) = pair();
        let server = Arc::new(BarrierServer::new(10));
        let thread_server = server.clone();
        let guard = thread::spawn(move || serve(server_streams, thread_server));
        let client: Arc<Client<Reply>> = Arc::new(Client::new(client_stream).unwrap());
        let mut join_handles = vec![];
        for _ in 0..10 {
            let my_client = client.clone();
            join_handles.push(thread::spawn(move || my_client.rpc(&Request::Increment).unwrap()));
        }
        for handle in join_handles.into_iter() {
            handle.join();
        }
        assert_eq!(10, server.count());
        let client = match Arc::try_unwrap(client) {
            Err(_) => panic!("couldn't unwrap arc"),
            Ok(c) => c,
        };
        client.join::<Request>().unwrap();
        guard.join();
    }
    */
}
