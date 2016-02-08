use bincode;
use serde;
use scoped_pool::Pool;
use std::fmt;
use std::io::{self, BufReader, BufWriter, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Condvar, Mutex};
use std::sync::mpsc::{Sender, TryRecvError, channel};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::thread::{self, JoinHandle};
use super::{Packet, Result};

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
                        inflight_rpcs.increment();
                        scope.execute(move || {
                            let reply = server.serve(message);
                            let reply_packet = Packet {
                                rpc_id: rpc_id,
                                message: reply
                            };
                            let mut write_stream = write_stream.lock().expect(pos!());
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
                        read_stream: BufReader::new(conn.try_clone().expect(pos!())),
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
