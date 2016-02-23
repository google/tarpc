use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// A transport for TCP.
pub struct TcpTransport<A: ToSocketAddrs>(pub A);
impl<A: ToSocketAddrs> super::Transport for TcpTransport<A> {
    type Listener = TcpListener;
    fn bind(&self) -> io::Result<TcpListener> {
        TcpListener::bind(&self.0)
    }
}

impl super::Listener for TcpListener {
    type Dialer = TcpDialer<SocketAddr>;
    type Stream = TcpStream;
    fn accept(&self) -> io::Result<TcpStream> {
        self.accept().map(|(stream, _)| stream)
    }
    fn dialer(&self) -> io::Result<TcpDialer<SocketAddr>> {
        self.local_addr().map(|addr| TcpDialer(addr))
    }
}

impl super::Stream for TcpStream {
    fn try_clone(&self) -> io::Result<Self> {
        self.try_clone()
    }
    fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(dur)
    }
    fn set_write_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.set_write_timeout(dur)
    }
    fn shutdown(&self) -> io::Result<()> {
        self.shutdown(::std::net::Shutdown::Both)
    }
}

/// Connects to a socket address.
pub struct TcpDialer<A = SocketAddr>(pub A)
    where A: ToSocketAddrs;
impl<A: ToSocketAddrs> super::Dialer for TcpDialer<A> {
    type Stream = TcpStream;
    type Addr = A;
    fn dial(&self) -> io::Result<TcpStream> {
        TcpStream::connect(&self.0)
    }
    fn addr(&self) -> &A {
        &self.0
    }
}
