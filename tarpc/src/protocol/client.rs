// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use serde;
use std::fmt;
use std::io::{self, BufReader, BufWriter, Read};
use std::collections::HashMap;
use std::mem;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Duration;

use super::{Deserialize, Error, Packet, Result, Serialize};

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
        self.outbound.send((request, tx)).expect(pos!());
        rx
    }

    /// Run the specified rpc method on the server this client is connected to
    pub fn rpc(&self, request: Request) -> Result<Reply>
        where Request: serde::ser::Serialize + fmt::Debug + Send + 'static
    {
        self.rpc_internal(request)
            .recv()
            .map_err(|_| self.requests.lock().expect(pos!()).get_error())
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
                warn!("Client: couldn't shutdown writer and reader threads: {:?}",
                      e);
            } else {
                // We only join if we know the TcpStream was shut down. Otherwise we might never
                // finish.
                debug!("Joining writer and reader.");
                reader_guard.take()
                            .expect(pos!())
                            .join()
                            .expect(pos!());
                debug!("Successfully joined writer and reader.");
            }
        }
    }
}

/// An asynchronous RPC call
pub struct Future<T> {
    rx: Receiver<Result<T>>,
    requests: Arc<Mutex<RpcFutures<T>>>,
}

impl<T> Future<T> {
    /// Block until the result of the RPC call is available
    pub fn get(self) -> Result<T> {
        let requests = self.requests;
        self.rx
            .recv()
            .map_err(|_| requests.lock().expect(pos!()).get_error())
            .and_then(|reply| reply)
    }
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

    fn complete_reply(&mut self, packet: Packet<Reply>) {
        if let Some(tx) = self.0.as_mut().expect(pos!()).remove(&packet.rpc_id) {
            if let Err(e) = tx.send(Ok(packet.message)) {
                info!("Reader: could not complete reply: {:?}", e);
            }
        } else {
            warn!("RpcFutures: expected sender for id {} but got None!",
                  packet.rpc_id);
        }
    }

    fn set_error(&mut self, err: Error) {
        let _ = mem::replace(&mut self.0, Err(err));
    }

    fn get_error(&self) -> Error {
        self.0.as_ref().err().expect(pos!()).clone()
    }
}

fn write<Request, Reply>(outbound: Receiver<(Request, Sender<Result<Reply>>)>,
                         requests: Arc<Mutex<RpcFutures<Reply>>>,
                         stream: TcpStream)
    where Request: serde::Serialize,
          Reply: serde::Deserialize
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
        if let Err(e) = requests.lock().expect(pos!()).insert_tx(next_id, tx.clone()) {
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
        debug!("Writer: writing rpc, id={:?}", id);
        if let Err(e) = stream.serialize(&packet) {
            report_error(&tx, e.into());
            // Typically we'd want to notify the client of any Err returned by remove_tx, but in
            // this case the client already hit an Err, and doesn't need to know about this one, as
            // well.
            let _ = requests.lock().expect(pos!()).remove_tx(id);
            continue;
        }
    }

    fn report_error<Reply>(tx: &Sender<Result<Reply>>, e: Error)
        where Reply: serde::Deserialize
    {
        // Clone the err so we can log it if sending fails
        if let Err(e2) = tx.send(Err(e.clone())) {
            debug!("Error encountered while trying to send an error. Initial error: {:?}; Send \
                    error: {:?}",
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
        match stream.deserialize::<Packet<Reply>>() {
            Ok(packet) => {
                debug!("Client: received message, id={}", packet.rpc_id);
                requests.lock().expect(pos!()).complete_reply(packet);
            }
            Err(err) => {
                warn!("Client: reader thread encountered an unexpected error while parsing; \
                       returning now. Error: {:?}",
                      err);
                requests.lock().expect(pos!()).set_error(err);
                break;
            }
        }
    }
}
