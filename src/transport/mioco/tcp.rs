use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use mioco;
use mioco::tcp::{TcpListener, TcpStream};
use std::time::Duration;
use super::super::{Dialer, Listener, Stream, Transport};

/// A transport for TCP.
#[derive(Debug)]
pub struct TcpTransport<A: ToSocketAddrs>(pub A);

impl<A: ToSocketAddrs> Transport for TcpTransport<A> {
    type Listener = TcpListener;

    fn bind(&self) -> io::Result<TcpListener> {
        TcpListener::bind(&try!(self.0.to_socket_addrs()).next().unwrap())
    }
}

impl Listener for TcpListener {
    type Dialer = TcpDialer<SocketAddr>;

    type Stream = TcpStream;

    fn accept(&self) -> io::Result<TcpStream> {
        self.accept()
    }

    fn dialer(&self) -> io::Result<TcpDialer<SocketAddr>> {
        self.local_addr().map(|addr| TcpDialer(addr))
    }
}

impl Stream for TcpStream {
    fn try_clone(&self) -> io::Result<Self> {
        self.try_clone()
    }

    fn set_read_timeout(&self, _dur: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    fn set_write_timeout(&self, _dur: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> io::Result<()> {
        self.shutdown(mioco::tcp::Shutdown::Both)
    }
}

/// Connects to a socket address.
#[derive(Debug)]
pub struct TcpDialer<A = SocketAddr>(pub A) where A: ToSocketAddrs;

impl<A> Dialer for TcpDialer<A>
    where A: ToSocketAddrs
{
    type Stream = TcpStream;

    fn dial(&self) -> io::Result<TcpStream> {
        TcpStream::connect(&try!(self.0.to_socket_addrs()).next().unwrap())
    }
}
