use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;
use unix_socket::{UnixListener, UnixStream};

/// A transport for unix sockets.
pub struct UnixTransport<P>(pub P)
    where P: AsRef<Path>;

impl<P> super::Transport for UnixTransport<P>
    where P: AsRef<Path>
{
    type Listener = UnixListener;
    fn bind(&self) -> io::Result<UnixListener> {
        UnixListener::bind(&self.0)
    }
}

/// Connects to a unix socket address.
pub struct UnixDialer<P>(pub P)
    where P: AsRef<Path>;

impl<P> super::Dialer for UnixDialer<P>
    where P: AsRef<Path>
{
    type Stream = UnixStream;
    type Addr = P;
    fn dial(&self) -> io::Result<UnixStream> {
        UnixStream::connect(&self.0)
    }
    fn addr(&self) -> &P {
        &self.0
    }
}

impl super::Listener for UnixListener {
    type Stream = UnixStream;
    type Dialer = UnixDialer<PathBuf>;
    fn accept(&self) -> io::Result<UnixStream> {
        self.accept().map(|(stream, _)| stream)
    }
    fn dialer(&self) -> io::Result<UnixDialer<PathBuf>> {
        self.local_addr().map(|addr| UnixDialer(addr.as_pathname().unwrap().to_owned()))
    }
}

impl super::Stream for UnixStream {
    fn try_clone(&self) -> io::Result<Self> {
        self.try_clone()
    }
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(timeout)
    }
    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.set_write_timeout(timeout)
    }
    fn shutdown(&self) -> io::Result<()> {
        self.shutdown(::std::net::Shutdown::Both)
    }
}
