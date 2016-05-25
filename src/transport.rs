// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use mio::{Evented, EventSet, PollOpt, Selector, Token};
use mio::tcp::{TcpListener, TcpStream};
use mio::unix::{PipeReader, PipeWriter, UnixListener, UnixSocket, UnixStream};
use std::convert::TryFrom;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use Error;

/// Contains a variant for each stream type supported by tarpc.
#[derive(Debug)]
pub enum Stream {
    /// Tcp stream.
    Tcp(TcpStream),
    #[cfg(unix)]
    /// Stdin / Stdout.
    Pipe(PipeWriter, PipeReader),
    #[cfg(unix)]
    /// Unix socket.
    Unix(UnixStream),
}

impl Read for Stream {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match *self {
            Stream::Tcp(ref mut stream) => stream.read(buf),
            #[cfg(unix)]
            Stream::Pipe(_, ref mut reader) => reader.read(buf),
            #[cfg(unix)]
            Stream::Unix(ref mut stream) => stream.read(buf),
        }
    }
}

impl Write for Stream {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match *self {
            Stream::Tcp(ref mut stream) => stream.write(buf),
            #[cfg(unix)]
            Stream::Pipe(ref mut writer, _) => writer.write(buf),
            #[cfg(unix)]
            Stream::Unix(ref mut stream) => stream.write(buf),
        }
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        match *self {
            Stream::Tcp(ref mut stream) => stream.flush(),
            #[cfg(unix)]
            Stream::Pipe(ref mut writer, _) => writer.flush(),
            #[cfg(unix)]
            Stream::Unix(ref mut stream) => stream.flush(),
        }
    }
}

impl Evented for Stream {
    #[inline]
    fn register(&self,
                poll: &mut Selector,
                token: Token,
                interest: EventSet,
                opts: PollOpt)
                -> io::Result<()> {
        match *self {
            Stream::Tcp(ref stream) => stream.register(poll, token, interest, opts),
            #[cfg(unix)]
            Stream::Pipe(ref writer, ref reader) => {
                try!(writer.register(poll, token, interest, opts));
                try!(reader.register(poll, token, interest, opts));
                Ok(())
            }
            #[cfg(unix)]
            Stream::Unix(ref stream) => stream.register(poll, token, interest, opts),
        }
    }

    #[inline]
    fn reregister(&self,
                  poll: &mut Selector,
                  token: Token,
                  interest: EventSet,
                  opts: PollOpt)
                  -> io::Result<()> {
        match *self {
            Stream::Tcp(ref stream) => stream.reregister(poll, token, interest, opts),
            #[cfg(unix)]
            Stream::Pipe(ref writer, ref reader) => {
                try!(writer.reregister(poll, token, interest, opts));
                try!(reader.reregister(poll, token, interest, opts));
                Ok(())
            }
            Stream::Unix(ref stream) => stream.reregister(poll, token, interest, opts),
        }
    }

    #[inline]
    fn deregister(&self, poll: &mut Selector) -> io::Result<()> {
        match *self {
            Stream::Tcp(ref stream) => stream.deregister(poll),
            #[cfg(unix)]
            Stream::Pipe(ref writer, ref reader) => {
                try!(writer.deregister(poll));
                try!(reader.deregister(poll));
                Ok(())
            }
            Stream::Unix(ref stream) => stream.deregister(poll),
        }
    }
}

impl TryFrom<Stream> for Stream {
    type Err = Error;
    fn try_from(stream: Stream) -> ::Result<Self> {
        Ok(stream)
    }
}

#[cfg(unix)]
impl<'a> TryFrom<&'a Path> for Stream {
    type Err = Error;

    fn try_from(path: &Path) -> ::Result<Self> {
        Ok(Stream::Unix(try!(try!(UnixSocket::stream()).connect(path)).0))
    }
}

#[cfg(unix)]
impl<'a> TryFrom<&'a PathBuf> for Stream {
    type Err = Error;

    fn try_from(path: &PathBuf) -> ::Result<Self> {
        Ok(Stream::Unix(try!(try!(UnixSocket::stream()).connect(path)).0))
    }
}

#[cfg(unix)]
impl TryFrom<PathBuf> for Stream {
    type Err = Error;

    fn try_from(path: PathBuf) -> ::Result<Self> {
        Stream::try_from(&path)
    }
}

#[cfg(unix)]
impl TryFrom<(PipeWriter, PipeReader)> for Stream {
    type Err = Error;

    fn try_from((tx, rx): (PipeWriter, PipeReader)) -> ::Result<Self> {
        Ok(Stream::Pipe(tx, rx))
    }
}

#[cfg(unix)]
impl From<(PipeWriter, PipeReader)> for Stream {
    fn from((tx, rx): (PipeWriter, PipeReader)) -> Self {
        Stream::Pipe(tx, rx)
    }
}

impl<'a> TryFrom<&'a str> for Stream {
    type Err = Error;

    fn try_from(addr: &str) -> ::Result<Self> {
        if let Some(addr) = try!(addr.to_socket_addrs()).next() {
            Ok(Stream::Tcp(try!(TcpStream::connect(&addr))))
        } else {
            Err(Error::NoAddressFound)
        }
    }
}

impl TryFrom<SocketAddr> for Stream {
    type Err = Error;

    fn try_from(addr: SocketAddr) -> ::Result<Self> {
        Stream::try_from(&addr)
    }
}

impl<'a> TryFrom<&'a SocketAddr> for Stream {
    type Err = Error;

    fn try_from(addr: &SocketAddr) -> ::Result<Self> {
        Ok(Stream::Tcp(try!(TcpStream::connect(&addr))))
    }
}

impl TryFrom<Option<SocketAddr>> for Stream {
    type Err = Error;

    fn try_from(addr: Option<SocketAddr>) -> ::Result<Self> {
        if let Some(addr) = addr {
            Ok(Stream::Tcp(try!(TcpStream::connect(&addr))))
        } else {
            Err(Error::NoAddressFound)
        }
    }
}

#[cfg(unix)]
impl TryFrom<UnixStream> for Stream {
    type Err = Error;

    fn try_from(stream: UnixStream) -> ::Result<Self> {
        Ok(Stream::Unix(stream))
    }
}

#[cfg(unix)]
impl From<UnixStream> for Stream {
    fn from(stream: UnixStream) -> Self {
        Stream::Unix(stream)
    }
}

impl From<TcpStream> for Stream {
    fn from(stream: TcpStream) -> Self {
        Stream::Tcp(stream)
    }
}

/// Encomposses various types that can listen for connecting clients.
#[derive(Debug)]
pub enum Listener {
    /// Tcp Listener
    Tcp(TcpListener),
    #[cfg(unix)]
    /// Unix socket listener
    Unix(UnixListener),
}

impl Listener {
    /// Accepts an incoming connection.
    pub fn accept(&self) -> ::Result<Option<Stream>> {
        match *self {
            Listener::Tcp(ref listener) => {
                Ok(try!(listener.accept()).map(|(stream, _)| stream.into()))
            }
            #[cfg(unix)]
            Listener::Unix(ref listener) => Ok(try!(listener.accept()).map(|stream| stream.into()))
        }
    }

    /// Returns the address the service is listening on, if any.
    pub fn local_addr(&self) -> ::Result<Option<SocketAddr>> {
        match *self {
            Listener::Tcp(ref listener) => Ok(Some(try!(listener.local_addr()))),
            #[cfg(unix)]
            Listener::Unix(_) => Ok(None),
        }
    }
}

impl<'a> TryFrom<&'a str> for Listener {
    type Err = Error;

    fn try_from(addr: &str) -> ::Result<Listener> {
        let addr = if let Some(addr) = try!(addr.to_socket_addrs()).next() {
            addr
        } else {
            return Err(Error::NoAddressFound);
        };
        Ok(Listener::Tcp(try!(TcpListener::bind(&addr))))
    }
}

impl TryFrom<SocketAddr> for Listener {
    type Err = Error;

    fn try_from(addr: SocketAddr) -> ::Result<Listener> {
        Listener::try_from(&addr)
    }
}

impl<'a> TryFrom<&'a SocketAddr> for Listener {
    type Err = Error;

    fn try_from(addr: &SocketAddr) -> ::Result<Listener> {
        Ok(Listener::Tcp(try!(TcpListener::bind(&addr))))
    }
}

#[cfg(unix)]
impl<'a> TryFrom<&'a Path> for Listener {
    type Err = Error;

    fn try_from(path: &Path) -> ::Result<Self> {
        Ok(Listener::Unix(try!(UnixListener::bind(path))))
    }
}

#[cfg(unix)]
impl<'a> TryFrom<&'a PathBuf> for Listener {
    type Err = Error;

    fn try_from(path: &PathBuf) -> ::Result<Self> {
        Ok(Listener::Unix(try!(UnixListener::bind(path))))
    }
}

#[cfg(unix)]
impl TryFrom<PathBuf> for Listener {
    type Err = Error;

    fn try_from(path: PathBuf) -> ::Result<Self> {
        Listener::try_from(&path)
    }
}

impl Evented for Listener {
    #[inline]
    fn register(&self,
                poll: &mut Selector,
                token: Token,
                interest: EventSet,
                opts: PollOpt)
                -> io::Result<()> {
        match *self {
            Listener::Tcp(ref listener) => listener.register(poll, token, interest, opts),
            #[cfg(unix)]
            Listener::Unix(ref listener) => listener.register(poll, token, interest, opts),
        }
    }

    #[inline]
    fn reregister(&self,
                  poll: &mut Selector,
                  token: Token,
                  interest: EventSet,
                  opts: PollOpt)
                  -> io::Result<()> {
        match *self {
            Listener::Tcp(ref listener) => listener.reregister(poll, token, interest, opts),
            #[cfg(unix)]
            Listener::Unix(ref listener) => listener.reregister(poll, token, interest, opts),
        }
    }

    #[inline]
    fn deregister(&self, poll: &mut Selector) -> io::Result<()> {
        match *self {
            Listener::Tcp(ref listener) => listener.deregister(poll),
            #[cfg(unix)]
            Listener::Unix(ref listener) => listener.deregister(poll),
        }
    }
}

