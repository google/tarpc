#![feature(custom_derive, plugin)]
#![plugin(serde_macros)]

extern crate serde;
extern crate serde_json;

use std::io;
use std::convert;
use std::collections::HashMap;
use std::error::Error as StdError;
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
    Sender,
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
        Error::Json(err)
    }
}

impl convert::From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(err)
    }
}

impl<T> convert::From<sync::mpsc::SendError<T>> for Error {
    fn from(err: sync::mpsc::SendError<T>) -> Error {
        Error::Sender
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn handle_conn<F, Request, Response>(mut conn: TcpStream, f: F) -> Result<()>
    where Request: serde::de::Deserialize,
          Response: serde::ser::Serialize,
          F: Fn(&Request) -> Result<Response>
{
    let request: Request = try!(serde_json::from_reader(&mut conn));
    let response = try!(f(&request));
    try!(serde_json::to_writer(&mut conn, &response));
    Ok(())
}

pub fn serve(listener: TcpListener) -> Error {
    for conn in listener.incoming() {
        let conn = match conn {
            Err(err) => return convert::From::from(err),
            Ok(c) => c,
        };
        thread::spawn(move || {
            if let Err(err) = handle_conn(conn, |a| handle_impl(a)) {
                println!("error handling connection: {:?}", err);
            }
        });
    }
    Error::Impossible
}

#[derive(Serialize, Deserialize)]
struct Packet<T> {
    seq: u64,
    message: T,
}

// Generated code

#[derive(Serialize, Deserialize)]
struct A;
#[derive(Serialize, Deserialize)]
struct B;

fn handle_impl(a: &A) -> Result<B> {
    Ok(B)
}

struct InnerClient {
    stream: TcpStream,
    seq: u64,
    outstanding_messages: HashMap<u64, Sender<()>>,
}

struct RPC<Request, Reply> {
    id: u64,
    request: Request,
    reply: Sender<Reply>,
}

struct RequestHandle<Request> {
    id: u64,
    request: Request,
}

struct ReplyHandle<Reply> {
    id: u64,
    reply: Sender<Reply>,
}

struct ReplyPacket<Reply> {
    id: u64,
    message: Reply,
}

fn message_reader<Reply>(
    mut stream: TcpStream,
    replies: Sender<ReceiverMessage<Reply>>) -> Result<()>
    where Reply: serde::de::Deserialize
{
    loop {
        let id = try!(serde_json::from_reader(&mut stream));
        let reply_message = try!(serde_json::from_reader(&mut stream));
        let packet = ReplyPacket{
            id: id,
            message: reply_message,
        };
        try!(replies.send(ReceiverMessage::Packet(packet)));
    }
}

enum ReceiverMessage<Reply> {
    Handle(ReplyHandle<Reply>),
    Packet(ReplyPacket<Reply>),
}

fn receiver<Reply>(messages: Receiver<ReceiverMessage<Reply>>) -> Result<()>
{
    let mut ready_handles: HashMap<u64, ReplyHandle<Reply>> = HashMap::new();
    let mut ready_packets: HashMap<u64, ReplyPacket<Reply>> = HashMap::new();
    for message in messages.into_iter() {
        match message {
            ReceiverMessage::Handle(handle) => {
                if let Some(packet) = ready_packets.remove(&handle.id) {
                    try!(handle.reply.send(packet.message));
                } else {
                    ready_handles.insert(handle.id, handle);
                }
            },
            ReceiverMessage::Packet(packet) => {
                if let Some(handle) = ready_handles.remove(&packet.id) {
                    try!(handle.reply.send(packet.message));
                } else {
                    ready_packets.insert(packet.id, packet);
                }
            }

        }
    }
    Ok(())
}

fn message_writer<Request>(
    mut stream: TcpStream,
    requests: Receiver<RequestHandle<Request>>) -> Result<()>
    where Request: serde::ser::Serialize
{
    for request_handle in requests.into_iter() {
        try!(serde_json::to_writer(&mut stream, &request_handle.id));
        try!(serde_json::to_writer(&mut stream, &request_handle.request));
    }
    Ok(())
}

struct Client<Request, Reply> {
    next_id: Mutex<u64>,
    writer_tx: Sender<RequestHandle<Request>>,
    handles_tx: Sender<ReceiverMessage<Reply>>,
}

impl<Request, Reply> Client<Request, Reply>
    where Request: serde::ser::Serialize + Clone + Send + 'static,
          Reply: serde::de::Deserialize + Send + 'static
{
    fn new(stream: TcpStream) -> Result<Self> {
        let write_stream = try!(stream.try_clone());
        let (requests_tx, requests_rx) = channel();
        let (handles_tx, receiver_rx) = channel();
        let replies_tx = handles_tx.clone();
        thread::spawn(move || message_writer(write_stream, requests_rx).unwrap());
        thread::spawn(move || message_reader(stream, replies_tx).unwrap());
        thread::spawn(move || receiver(receiver_rx).unwrap());
        Ok(Client{
            next_id: Mutex::new(0),
            writer_tx: requests_tx,
            handles_tx: handles_tx,
        })
    }

    fn get_next_id(&self) -> u64 {
        let mut id = self.next_id.lock().unwrap();
        *id += 1;
        *id
    }

    fn rpc(&self, request: &Request) -> Result<Reply> {
        let (tx, rx) = channel();
        let id = self.get_next_id();
        try!(self.writer_tx.send(RequestHandle{
            id: id,
            request: request.clone(),
        }));
        try!(self.handles_tx.send(ReceiverMessage::Handle(ReplyHandle{
            id: id,
            reply: tx,
        })));
        Ok(rx.recv().unwrap())
    }
}

/*
#[cfg(test)]
mod test {
    use adamrpc::*;

    #[test]
    fn test() {
        let listener = TcpListener::bind("127.0.0.1:9000").expect("listener");
        let server = 
        let stream = TcpStream::connect
    }
}
*/
