use std::io;
use tokio_core::io::Io;
use tokio_core::net::TcpStream;
#[cfg(feature = "tls")]
use tokio_tls::TlsStream;

#[derive(Debug)]
pub enum StreamType {
    Tcp(TcpStream),
    #[cfg(feature = "tls")]
    Tls(TlsStream<TcpStream>),
}

impl From<TcpStream> for StreamType {
    fn from(stream: TcpStream) -> Self {
        StreamType::Tcp(stream)
    }
}

#[cfg(feature = "tls")]
impl From<TlsStream<TcpStream>> for StreamType {
    fn from(stream: TlsStream<TcpStream>) -> Self {
        StreamType::Tls(stream)
    }
}

impl io::Read for StreamType {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match *self {
            StreamType::Tcp(ref mut stream) => stream.read(buf),
            #[cfg(feature = "tls")]
            StreamType::Tls(ref mut stream) => stream.read(buf),
        }
    }
}

impl io::Write for StreamType {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match *self {
            StreamType::Tcp(ref mut stream) => stream.write(buf),
            #[cfg(feature = "tls")]
            StreamType::Tls(ref mut stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match *self {
            StreamType::Tcp(ref mut stream) => stream.flush(),
            #[cfg(feature = "tls")]
            StreamType::Tls(ref mut stream) => stream.flush(),
        }
    }
}

impl Io for StreamType {}
