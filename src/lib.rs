#![feature(custom_derive, plugin)]
#![plugin(serde_macros)]

extern crate multi_tcp;
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
};
use std::sync::mpsc::{
    channel,
    sync_channel,
    Sender,
    SyncSender,
    Receiver,
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

pub fn handle_conn<F, Request, Reply>(
    mut stream: TcpStream,
    f: F) -> Result<()>
    where Request: fmt::Debug + serde::de::Deserialize,
          Reply: fmt::Debug + serde::ser::Serialize,
          F: 'static + Serve<Request, Reply>
{
    let read_stream = try!(stream.try_clone());
    let mut de = serde_json::Deserializer::new(read_stream.bytes());
    let request_packet: Packet<Request> = try!(Packet::deserialize(&mut de));
    let reply = try!(f.serve(&request_packet.message));
    let reply_packet = Packet{
        id: request_packet.id,
        message: reply,
    };
    try!(serde_json::to_writer(&mut stream, &reply_packet));
    Ok(())
}

pub fn serve<F, Request, Reply>(listener: TcpListener, f: F) -> Error
    where Request: fmt::Debug + serde::de::Deserialize,
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

pub trait Serve<Request, Reply> : Sync + Send + Clone {
    fn serve(&self, request: &Request) -> io::Result<Reply>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Packet<T> {
    id: u64,
    message: T,
}

struct Handle<T> {
    id: u64,
    sender: Sender<T>,
}

enum ReceiverMessage<Reply> {
    Handle(Handle<Reply>),
    Packet(Packet<Reply>),
}

fn receiver<Reply>(messages: Receiver<ReceiverMessage<Reply>>) -> Result<()> {
    let mut ready_handles: HashMap<u64, Handle<Reply>> = HashMap::new();
    for message in messages.into_iter() {
        match message {
            ReceiverMessage::Handle(handle) => {
                ready_handles.insert(handle.id, handle);
            },
            ReceiverMessage::Packet(packet) => {
                let handle = ready_handles.remove(&packet.id).unwrap();
                try!(handle.sender.send(packet.message));
            }
        }
    }
    Ok(())
}

pub struct Client<Request, Reply> {
    next_id: Mutex<u64>,
    writer: multi_tcp::MultiStream<Packet<Request>, Error>,
    handles_tx: SyncSender<ReceiverMessage<Reply>>,
}

impl<Request, Reply> Client<Request, Reply>
    where Request: serde::ser::Serialize + Clone + Send + 'static,
          Reply: serde::de::Deserialize + Send + 'static
{
    pub fn new(stream: TcpStream) -> Result<Self> {
        let (handles_tx, receiver_rx) = sync_channel(0);
        let writer: multi_tcp::MultiStream<Packet<Request>, Error>
            = multi_tcp::MultiStream::with_sync_sender(
            stream,
            |stream: &mut TcpStream, packet: &Packet<Request>| {
                try!(serde_json::to_writer(stream, packet));
                Ok(())
            },
            |mut stream| {
                let packet = try!(serde_json::from_reader(&mut stream));
                Ok(ReceiverMessage::Packet(packet))
            },
            handles_tx.clone());
        thread::spawn(move || receiver(receiver_rx));
        Ok(Client{
            next_id: Mutex::new(0),
            writer: writer,
            handles_tx: handles_tx,
        })
    }

    fn get_next_id(&self) -> u64 {
        let mut id = self.next_id.lock().unwrap();
        *id += 1;
        *id
    }

    pub fn rpc(&self, request: &Request) -> Result<Reply> {
        let (tx, rx) = channel();
        let id = self.get_next_id();
        try!(self.handles_tx.send(ReceiverMessage::Handle(Handle{
            id: id,
            sender: tx,
        })));
        try!(self.writer.write(Packet{
            id: id,
            message: request.clone(),
        }));
        Ok(rx.recv().unwrap())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::thread;
    use std::net::{TcpStream, TcpListener};
    use std::io;

    fn pair() -> (TcpStream, TcpListener) {
        let addr = "127.0.0.1:9000";
        // Do this one first so that we don't get connection refused :)
        let listener = TcpListener::bind(addr).unwrap();
        (TcpStream::connect(addr).unwrap(), listener)
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
    enum Request {
        Increment
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum Reply {
        Increment
    }

    #[derive(Clone)]
    struct Server;

    impl Serve<Request, Reply> for Server {
        fn serve(&self, _: &Request) -> io::Result<Reply> {
            Ok(Reply::Increment)
        }
    }

    #[test]
    fn test() {
        let (client_stream, server_streams) = pair();
        thread::spawn(|| serve(server_streams, Server));
        let client: Client<Request, Reply> = Client::new(client_stream).unwrap();
        assert_eq!(Reply::Increment, client.rpc(&Request::Increment).unwrap());
    }
}
