// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode;
use serde;
use scoped_pool::{Pool, Scope};
use std::fmt;
use std::io::{self, BufReader, BufWriter, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::thread::{self, JoinHandle};
use super::{Packet, Result};

struct ConnectionHandler<'a, S>
    where S: Serve
{
    read_stream: BufReader<TcpStream>,
    write_stream: BufWriter<TcpStream>,
    server: S,
    shutdown: &'a AtomicBool,
}

impl<'a, S> ConnectionHandler<'a, S>
    where S: Serve
{
    fn handle_conn<'b>(&'b mut self, scope: &Scope<'b>) -> Result<()> {
        let ConnectionHandler {
            ref mut read_stream,
            ref mut write_stream,
            ref server,
            shutdown,
        } = *self;
        trace!("ConnectionHandler: serving client...");
        let (tx, rx) = channel();
        scope.execute(move || Self::write(rx, write_stream));
        loop {
            match bincode::serde::deserialize_from(read_stream, bincode::SizeLimit::Infinite) {
                Ok(Packet { rpc_id, message, }) => {
                    let tx = tx.clone();
                    scope.execute(move || {
                        let reply = server.serve(message);
                        let reply_packet = Packet {
                            rpc_id: rpc_id,
                            message: reply
                        };
                        tx.send(reply_packet).expect(pos!());
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
    }

    fn timed_out(error_kind: io::ErrorKind) -> bool {
        match error_kind {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => true,
            _ => false,
        }
    }

    fn write(rx: Receiver<Packet<<S as Serve>::Reply>>, stream: &mut BufWriter<TcpStream>) {
        loop {
            match rx.recv() {
                Err(e) => {
                    debug!("Write thread: returning due to {:?}", e);
                    return;
                }
                Ok(reply_packet) => {
                    if let Err(e) =
                           bincode::serde::serialize_into(stream,
                                                          &reply_packet,
                                                          bincode::SizeLimit::Infinite) {
                        warn!("Writer: failed to write reply to Client: {:?}",
                              e);
                    }
                    if let Err(e) = stream.flush() {
                        warn!("Writer: failed to flush reply to Client: {:?}",
                              e);
                    }
                }
            }
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
        self.join_handle.join().expect(pos!());
    }

    /// Returns the address the server is bound to
    pub fn local_addr(&self) -> &SocketAddr {
        &self.addr
    }

    /// Shutdown the server. Gracefully shuts down the serve thread but currently does not
    /// gracefully close open connections.
    pub fn shutdown(self) {
        info!("ServeHandle: attempting to shut down the server.");
        self.tx.send(()).expect(pos!());
        if let Ok(_) = TcpStream::connect(self.addr) {
            self.join_handle.join().expect(pos!());
        } else {
            warn!("ServeHandle: best effort shutdown of serve thread failed");
        }
    }
}

struct Server<'a, S: 'a> {
    server: &'a S,
    listener: TcpListener,
    read_timeout: Option<Duration>,
    die_rx: Receiver<()>,
    shutdown: &'a AtomicBool,
}

impl<'a, S: 'a> Server<'a, S>
    where S: Serve + 'static
{
    fn serve<'b>(self, scope: &Scope<'b>) where 'a: 'b {
        for conn in self.listener.incoming() {
            match self.die_rx.try_recv() {
                Ok(_) => {
                    info!("serve: shutdown received.");
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    info!("serve: shutdown sender disconnected.");
                    return;
                }
                _ => (),
            }
            let conn = match conn {
                Err(err) => {
                    error!("serve: failed to accept connection: {:?}", err);
                    return;
                }
                Ok(c) => c,
            };
            if let Err(err) = conn.set_read_timeout(self.read_timeout) {
                info!("serve: could not set read timeout: {:?}", err);
                continue;
            }
            let read_conn = match conn.try_clone() {
                Err(err) => {
                    error!("serve: could not clone tcp stream; possibly out of file descriptors? \
                           Err: {:?}",
                           err);
                    continue;
                }
                Ok(conn) => conn,
            };
            let mut handler = ConnectionHandler {
                read_stream: BufReader::new(read_conn),
                write_stream: BufWriter::new(conn),
                server: self.server,
                shutdown: self.shutdown,
            };
            scope.recurse(move |scope| {
                scope.zoom(|scope| {
                    if let Err(err) = handler.handle_conn(scope) {
                        info!("ConnectionHandler: err in connection handling: {:?}", err);
                    }
                });
            });
        }
    }
}

impl<'a, S> Drop for Server<'a, S> {
    fn drop(&mut self) {
        debug!("Shutting down connection handlers.");
        self.shutdown.store(true, Ordering::SeqCst);
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
        let server = Server {
            server: &server,
            listener: listener,
            read_timeout: read_timeout,
            die_rx: die_rx,
            shutdown: &shutdown,
        };
        pool.scoped(|scope| {
            server.serve(scope);
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
    type Reply  : 'static + fmt::Debug + serde::ser::Serialize + serde::de::Deserialize + Send;

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
