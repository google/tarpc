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
        Incoming { listener: self }
    }
}

/// A cloneable Reader/Writer.
pub trait Stream: Read + Write + Send + Sized + 'static {
    /// Creates a new independently owned handle to the underlying socket.
    ///
    /// The returned TcpStream should reference the same stream that this
    /// object references. Both handles should read and write the same
    /// stream of data, and options set on one stream should be propagated
    /// to the other stream.
    fn try_clone(&self) -> io::Result<Self>;
    /// Sets a read timeout.
    ///
    /// If the value specified is `None`, then read calls will block indefinitely.
    /// It is an error to pass the zero `Duration` to this method.
    fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()>;
    /// Sets a write timeout.
    ///
    /// If the value specified is `None`, then write calls will block indefinitely.
    /// It is an error to pass the zero `Duration` to this method.
    fn set_write_timeout(&self, dur: Option<Duration>) -> io::Result<()>;
    /// Shuts down both ends of the stream.
    ///
    /// Implementations should cause all pending and future I/O on the specified
    /// portions to return immediately with an appropriate value.
    fn shutdown(&self) -> io::Result<()>;
}

/// A `Stream` factory.
pub trait Dialer {
    /// The type of `Stream` this can create.
    type Stream: Stream;
    /// Open a stream.
    fn dial(&self) -> io::Result<Self::Stream>;
}

impl<P, D: ?Sized> Dialer for P
    where P: ::std::ops::Deref<Target = D>,
          D: Dialer + 'static
{
    type Stream = D::Stream;
    fn dial(&self) -> io::Result<Self::Stream> {
        (**self).dial()
    }
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
