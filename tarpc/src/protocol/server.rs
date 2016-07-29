// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use serde;
use scoped_pool::{Pool, Scope};
use std::fmt;
use std::io::{self, BufReader, BufWriter};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::thread::{self, JoinHandle};
use super::{Config, Deserialize, Error, Packet, Result, Serialize};
use transport::{Dialer, Listener, Stream, Transport};
use transport::tcp::TcpDialer;

struct ConnectionHandler<'a, S, St>
    where S: Serve,
          St: Stream
{
    read_stream: BufReader<St>,
    write_stream: BufWriter<St>,
    server: S,
    shutdown: &'a AtomicBool,
}

impl<'a, S, St> ConnectionHandler<'a, S, St>
    where S: Serve,
          St: Stream
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
            match read_stream.deserialize() {
                Ok(Packet { rpc_id, message }) => {
                    let tx = tx.clone();
                    scope.execute(move || {
                        let reply = server.serve(message);
                        let reply_packet = Packet {
                            rpc_id: rpc_id,
                            message: reply,
                        };
                        tx.send(reply_packet).expect(pos!());
                    });
                    if shutdown.load(Ordering::SeqCst) {
                        info!("ConnectionHandler: server shutdown, so closing connection.");
                        break;
                    }
                }
                Err(Error::Io(ref err)) if Self::timed_out(err.kind()) => {
                    if !shutdown.load(Ordering::SeqCst) {
                        info!("ConnectionHandler: read timed out ({:?}). Server not shutdown, so \
                               retrying read.",
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
            io::ErrorKind::TimedOut |
            io::ErrorKind::WouldBlock => true,
            _ => false,
        }
    }

    fn write(rx: Receiver<Packet<<S as Serve>::Reply>>, stream: &mut BufWriter<St>) {
        loop {
            match rx.recv() {
                Err(e) => {
                    debug!("Write thread: returning due to {:?}", e);
                    return;
                }
                Ok(reply_packet) => {
                    if let Err(e) = stream.serialize(&reply_packet) {
                        warn!("Writer: failed to write reply to Client: {:?}", e);
                    }
                }
            }
        }
    }
}

/// Provides methods for blocking until the server completes,
pub struct ServeHandle<D = TcpDialer>
    where D: Dialer
{
    tx: Sender<()>,
    join_handle: JoinHandle<()>,
    dialer: D,
}

impl<D> ServeHandle<D>
    where D: Dialer
{
    /// Block until the server completes
    pub fn wait(self) {
        self.join_handle.join().expect(pos!());
    }

    /// Returns the dialer to the server.
    pub fn dialer(&self) -> &D {
        &self.dialer
    }

    /// Shutdown the server. Gracefully shuts down the serve thread but currently does not
    /// gracefully close open connections.
    pub fn shutdown(self) {
        info!("ServeHandle: attempting to shut down the server.");
        self.tx.send(()).expect(pos!());
        if let Ok(_) = self.dialer.dial() {
            self.join_handle.join().expect(pos!());
        } else {
            warn!("ServeHandle: best effort shutdown of serve thread failed");
        }
    }
}

struct Server<'a, S: 'a, L>
    where L: Listener
{
    server: &'a S,
    listener: L,
    read_timeout: Option<Duration>,
    die_rx: Receiver<()>,
    shutdown: &'a AtomicBool,
}

impl<'a, S, L> Server<'a, S, L>
    where S: Serve + 'static,
          L: Listener
{
    fn serve<'b>(self, scope: &Scope<'b>)
        where 'a: 'b
    {
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

impl<'a, S, L> Drop for Server<'a, S, L>
    where L: Listener
{
    fn drop(&mut self) {
        debug!("Shutting down connection handlers.");
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

/// A service provided by a server
pub trait Serve: Send + Sync + Sized {
    /// The type of request received by the server
    type Request: 'static + fmt::Debug + serde::ser::Serialize + serde::de::Deserialize + Send;
    /// The type of reply sent by the server
    type Reply: 'static + fmt::Debug + serde::ser::Serialize + serde::de::Deserialize + Send;

    /// Return a reply for a given request
    fn serve(&self, request: Self::Request) -> Self::Reply;

    /// spawn
    fn spawn<T>(self, transport: T) -> io::Result<ServeHandle<<T::Listener as Listener>::Dialer>>
        where T: Transport,
              Self: 'static
    {
        self.spawn_with_config(transport, Config::default())
    }

    /// spawn
    fn spawn_with_config<T>(self,
                            transport: T,
                            config: Config)
                            -> io::Result<ServeHandle<<T::Listener as Listener>::Dialer>>
        where T: Transport,
              Self: 'static
    {
        let listener = try!(transport.bind());
        let dialer = try!(listener.dialer());
        info!("spawn_with_config: spinning up server.");
        let (die_tx, die_rx) = channel();
        let timeout = config.timeout;
        let join_handle = thread::spawn(move || {
            let pool = Pool::new(100); // TODO(tjk): make this configurable, and expire idle threads
            let shutdown = AtomicBool::new(false);
            let server = Server {
                server: &self,
                listener: listener,
                read_timeout: timeout,
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
            dialer: dialer,
        })
    }
}

impl<P, S> Serve for P
    where P: Send + Sync + ::std::ops::Deref<Target = S>,
          S: Serve
{
    type Request = S::Request;
    type Reply = S::Reply;

    fn serve(&self, request: S::Request) -> S::Reply {
        S::serve(self, request)
    }
}
