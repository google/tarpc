use bytes::Buf;
use futures::Poll;
use std::io;
use tokio_core::net::TcpStream;
use tokio_io::{AsyncRead, AsyncWrite};
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

impl AsyncRead for StreamType {
    unsafe fn prepare_uninitialized_buffer(&self, buf: &mut [u8]) -> bool {
        match *self {
            StreamType::Tcp(ref stream) => stream.prepare_uninitialized_buffer(buf),
            #[cfg(feature = "tls")]
            StreamType::Tls(ref stream) => stream.prepare_uninitialized_buffer(buf),
        }
    }
}

impl AsyncWrite for StreamType {
    fn shutdown(&mut self) -> Poll<(), io::Error> {
        match *self {
            StreamType::Tcp(ref mut stream) => stream.shutdown(),
            #[cfg(feature = "tls")]
            StreamType::Tls(ref mut stream) => stream.shutdown(),
        }
    }

    fn write_buf<B: Buf>(&mut self, buf: &mut B) -> Poll<usize, io::Error> {
        match *self {
            StreamType::Tcp(ref mut stream) => stream.write_buf(buf),
            #[cfg(feature = "tls")]
            StreamType::Tls(ref mut stream) => stream.write_buf(buf),
        }
    }
}
