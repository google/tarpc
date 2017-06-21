

use super::{AlwaysOkUnit, connection};
use futures::{Async, Future, Poll, Stream, future as futures, stream};
use futures::sync::{mpsc, oneshot};
use futures::unsync;

/// A hook to shut down a running server.
#[derive(Clone, Debug)]
pub struct Shutdown {
    tx: mpsc::UnboundedSender<oneshot::Sender<()>>,
}

/// A future that resolves when server shutdown completes.
#[derive(Debug)]
pub struct ShutdownFuture {
    inner: futures::Either<futures::FutureResult<(), ()>, AlwaysOkUnit<oneshot::Receiver<()>>>,
}

impl Future for ShutdownFuture {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<(), ()> {
        self.inner.poll()
    }
}

impl Shutdown {
    /// Initiates an orderly server shutdown.
    ///
    /// First, the server enters lameduck mode, in which
    /// existing connections are honored but no new connections are accepted. Then, once all
    /// connections are closed, it initates total shutdown.
    ///
    /// The returned future resolves when the server is completely shut down.
    pub fn shutdown(&self) -> ShutdownFuture {
        let (tx, rx) = oneshot::channel();
        let inner = if let Err(_) = self.tx.send(tx) {
            trace!("Server already initiated shutdown.");
            futures::Either::A(futures::ok(()))
        } else {
            futures::Either::B(AlwaysOkUnit(rx))
        };
        ShutdownFuture { inner: inner }
    }
}

#[derive(Debug)]
pub struct Watcher {
    shutdown_rx: stream::Take<mpsc::UnboundedReceiver<oneshot::Sender<()>>>,
    connections: unsync::mpsc::UnboundedReceiver<connection::Action>,
    queued_error: Option<()>,
    shutdown: Option<oneshot::Sender<()>>,
    done: bool,
    num_connections: u64,
}

impl Watcher {
    pub fn triple() -> (connection::Tracker, Shutdown, Self) {
        let (connection_tx, connections) = connection::Tracker::pair();
        let (shutdown_tx, shutdown_rx) = mpsc::unbounded();
        (
            connection_tx,
            Shutdown { tx: shutdown_tx },
            Watcher {
                shutdown_rx: shutdown_rx.take(1),
                connections: connections,
                queued_error: None,
                shutdown: None,
                done: false,
                num_connections: 0,
            },
        )
    }

    fn process_connection(&mut self, action: connection::Action) {
        match action {
            connection::Action::Increment => self.num_connections += 1,
            connection::Action::Decrement => self.num_connections -= 1,
        }
    }

    fn poll_shutdown_requests(&mut self) -> Poll<Option<()>, ()> {
        Ok(Async::Ready(match try_ready!(self.shutdown_rx.poll()) {
            Some(tx) => {
                debug!("Received shutdown request.");
                self.shutdown = Some(tx);
                Some(())
            }
            None => None,
        }))
    }

    fn poll_connections(&mut self) -> Poll<Option<()>, ()> {
        Ok(Async::Ready(match try_ready!(self.connections.poll()) {
            Some(action) => {
                self.process_connection(action);
                Some(())
            }
            None => None,
        }))
    }

    fn poll_shutdown_requests_and_connections(&mut self) -> Poll<Option<()>, ()> {
        if let Some(e) = self.queued_error.take() {
            return Err(e);
        }

        match try!(self.poll_shutdown_requests()) {
            Async::NotReady => {
                match try_ready!(self.poll_connections()) {
                    Some(()) => Ok(Async::Ready(Some(()))),
                    None => Ok(Async::NotReady),
                }
            }
            Async::Ready(None) => {
                match try_ready!(self.poll_connections()) {
                    Some(()) => Ok(Async::Ready(Some(()))),
                    None => Ok(Async::Ready(None)),
                }
            }
            Async::Ready(Some(())) => {
                match self.poll_connections() {
                    Err(e) => {
                        self.queued_error = Some(e);
                        Ok(Async::Ready(Some(())))
                    }
                    Ok(Async::NotReady) | Ok(Async::Ready(None)) | Ok(Async::Ready(Some(()))) => {
                        Ok(Async::Ready(Some(())))
                    }
                }
            }
        }
    }

    fn should_continue(&mut self) -> bool {
        match self.shutdown.take() {
            Some(shutdown) => {
                debug!("Lameduck mode: {} open connections", self.num_connections);
                if self.num_connections == 0 {
                    debug!("Shutting down.");
                    // Not required for the shutdown future to be waited on, so this
                    // can fail (which is fine).
                    let _ = shutdown.send(());
                    false
                } else {
                    self.shutdown = Some(shutdown);
                    true
                }
            }
            None => true,
        }
    }

    fn process_request(&mut self) -> Poll<Option<()>, ()> {
        if self.done {
            return Ok(Async::Ready(None));
        }
        if self.should_continue() {
            self.poll_shutdown_requests_and_connections()
        } else {
            self.done = true;
            Ok(Async::Ready(None))
        }
    }
}

impl Future for Watcher {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<(), ()> {
        loop {
            match try!(self.process_request()) {
                Async::Ready(Some(())) => continue,
                Async::Ready(None) => return Ok(Async::Ready(())),
                Async::NotReady => return Ok(Async::NotReady),
            }
        }
    }
}
