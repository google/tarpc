use std::io::{self, Read, Write};
use std::time::Duration;

/// A factory for creating a listener on a given address.
pub trait Transport {
    /// The type of listener that binds to the given address.
    type Listener: Listener;
    /// Return a listener on the given address, and a dialer to that address.
    fn bind(&self) -> io::Result<Self::Listener>;
}

/// Accepts incoming connections from dialers.
pub trait Listener: Send + 'static {
    /// The type of address being listened on.
    type Dialer: Dialer;
    /// The type of stream this listener accepts.
    type Stream: Stream;
    /// Accept an incoming stream.
    fn accept(&self) -> io::Result<Self::Stream>;
    /// Returns the local address being listened on.
    fn dialer(&self) -> io::Result<Self::Dialer>;
    /// Iterate over incoming connections.
    fn incoming(&self) -> Incoming<Self> {
        Incoming {
            listener: self,
        }
    }
}

/// A cloneable Reader/Writer.
pub trait Stream: Read + Write + Send + Sized + 'static {
    /// Clone that can fail.
    fn try_clone(&self) -> io::Result<Self>;
    /// Sets a read timeout.
    fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()>;
    /// Sets a write timeout.
    fn set_write_timeout(&self, dur: Option<Duration>) -> io::Result<()>;
    /// Shuts down both ends of the stream.
    fn shutdown(&self) -> io::Result<()>;
}

/// A `Stream` factory.
pub trait Dialer {
    /// The type of `Stream` this can create.
    type Stream: Stream;
    /// The type of address being connected to.
    type Addr;
    /// Open a stream.
    fn dial(&self) -> io::Result<Self::Stream>;
    /// Return the address being dialed.
    fn addr(&self) -> &Self::Addr;
}

/// Iterates over incoming connections.
pub struct Incoming<'a, L: Listener + ?Sized + 'a> {
    listener: &'a L,
}

impl<'a, L: Listener> Iterator for Incoming<'a, L> {
    type Item = io::Result<L::Stream>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.listener.accept())
    }
}

/// Provides a TCP transport.
pub mod tcp;
/// Provides a unix socket transport.
pub mod unix;
