// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode::SizeLimit;
use bincode::serde as bincode;
use mio::{EventSet, Evented, PollOpt, Selector, Token};
use mio::tcp::TcpStream;
use mio::unix::{PipeReader, PipeWriter, UnixStream};
use serde;
use std::convert::{TryFrom, TryInto};
use std::io::{self, Cursor, Read, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::rc::Rc;
use Error;

/// Client-side implementation of the tarpc protocol.
pub mod client;

/// Server-side implementation of the tarpc protocol.
pub mod server;

mod reader;
mod writer;

type ReadState = self::reader::ReadState;
type WriteState<D> = self::writer::WriteState<D>;

id_wrapper!(RpcId);

/// Contains a variant for each stream type supported by tarpc.
#[derive(Debug)]
pub enum Stream {
    /// Tcp stream.
    Tcp(TcpStream),
    /// Stdin / Stdout.
    Pipe(PipeWriter, PipeReader),
    /// Unix socket.
    Unix(UnixStream),
}

impl<'a, S> TryFrom<&'a S> for Stream
    where S: TryInto<Stream, Err=Error>
{
    type Err = Error;
    fn try_from(s: &S) -> ::Result<Self> {
        s.try_into()
    }
}

impl TryFrom<Stream> for Stream {
    type Err = Error;
    fn try_from(stream: Stream) -> ::Result<Self> {
        Ok(stream)
    }
}

impl TryFrom<(PipeWriter, PipeReader)> for Stream {
    type Err = Error;

    fn try_from((tx, rx): (PipeWriter, PipeReader)) -> ::Result<Self> {
        Ok(Stream::Pipe(tx, rx))
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
        Ok(Stream::Tcp(try!(TcpStream::connect(&addr))))
    }
}

impl Read for Stream {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match *self {
            Stream::Tcp(ref mut stream) => stream.read(buf),
            Stream::Pipe(_, ref mut reader) => reader.read(buf),
            Stream::Unix(ref mut stream) => stream.read(buf),
        }
    }
}

impl Write for Stream {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match *self {
            Stream::Tcp(ref mut stream) => stream.write(buf),
            Stream::Pipe(ref mut writer, _) => writer.write(buf),
            Stream::Unix(ref mut stream) => stream.write(buf),
        }
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        match *self {
            Stream::Tcp(ref mut stream) => stream.flush(),
            Stream::Pipe(ref mut writer, _) => writer.flush(),
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
            Stream::Pipe(ref writer, ref reader) => {
                try!(writer.register(poll, token, interest, opts));
                try!(reader.register(poll, token, interest, opts));
                Ok(())
            }
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
            Stream::Pipe(ref writer, ref reader) => {
                try!(writer.deregister(poll));
                try!(reader.deregister(poll));
                Ok(())
            }
            Stream::Unix(ref stream) => stream.deregister(poll),
        }
    }
}

/// Something that can tell you its length.
trait Len {
    /// The length of the container.
    fn len(&self) -> usize;
}

impl<L> Len for Rc<L>
    where L: Len
{
    #[inline]
    fn len(&self) -> usize {
        (&**self).len()
    }
}

impl Len for Vec<u8> {
    #[inline]
    fn len(&self) -> usize {
        self.len()
    }
}

impl Len for [u8; 8] {
    #[inline]
    fn len(&self) -> usize {
        8
    }
}

/// The means of communication between client and server.
#[derive(Clone, Debug)]
pub struct Packet<D> {
    /// Identifies the request. The reply packet should specify the same id as the request.
    pub id: RpcId,
    /// The payload is typically a message that the client and server deserializes
    /// before handling.
    pub payload: D,
}

/// Serialize `s`. Returns `Vec<u8>` if successful, otherwise `tarpc::Error`.
pub fn serialize<S: serde::Serialize>(s: &S) -> ::Result<Vec<u8>> {
    bincode::serialize(s, SizeLimit::Infinite).map_err(|e| e.into())
}

/// Deserialize a buffer into a `D`. On error, returns `tarpc::Error`.
pub fn deserialize<D: serde::Deserialize>(buf: &Vec<u8>) -> ::Result<D> {
    bincode::deserialize_from(&mut Cursor::new(buf), SizeLimit::Infinite).map_err(|e| e.into())
}

#[cfg(test)]
mod test {
    extern crate env_logger;
    use ::client::AsyncClient;
    use ::server::{self, AsyncService, GenericCtx};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct AsyncServer {
        counter: Arc<AtomicUsize>,
    }

    impl AsyncService for AsyncServer {
        fn handle(&mut self, ctx: GenericCtx, _: Vec<u8>) {
            ctx.reply(Ok(self.counter.load(Ordering::SeqCst) as u64)).unwrap();
            self.counter.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl AsyncServer {
        fn new() -> AsyncServer {
            AsyncServer { counter: Arc::new(AtomicUsize::new(0)) }
        }
    }

    #[test]
    fn simple() {
        let _ = env_logger::init();
        let server = AsyncServer::new();
        let count = server.counter.clone();
        let serve_handle =
            server::AsyncServer::listen("localhost:0", server, ::server::Config::default())
                .expect(pos!());
        // The explicit type is required so that it doesn't deserialize a u32 instead of u64
        let client = AsyncClient::connect(serve_handle.local_addr()).expect(pos!());
        assert_eq!(0u64, client.rpc_sync(&()).expect(pos!()));
        assert_eq!(1, count.load(Ordering::SeqCst));
        assert_eq!(1u64, client.rpc_sync(&()).expect(pos!()));
        assert_eq!(2, count.load(Ordering::SeqCst));
    }

    #[test]
    fn client_failed_rpc() {
        let _ = env_logger::init();
        let server = server::AsyncServer::new("localhost:0", AsyncServer::new()).unwrap();
        let registry = server::Dispatcher::spawn().unwrap();
        let serve_handle = registry.register(server).unwrap();
        let client = AsyncClient::connect(serve_handle.local_addr()).unwrap();
        info!("Rpc 1");
        client.rpc_sync::<_, u64>(&()).unwrap();
        info!("Shutting down server...");
        registry.shutdown().unwrap();
        info!("Rpc 2");
        match client.rpc_sync::<_, u64>(&()) {
            Err(::Error::ConnectionBroken) => {}
            otherwise => panic!("Expected Err(ConnectionBroken), got {:?}", otherwise),
        }
        info!("Rpc 3");
        if let Ok(..) = client.rpc_sync::<_, u64>(&()) {
            // Test whether second failure hangs
            panic!("Should not be able to receive a successful rpc after ConnectionBroken.");
        }
    }

    #[test]
    fn async() {
        let _ = env_logger::init();
        let server = AsyncServer::new();
        let serve_handle =
            server::AsyncServer::listen("localhost:0", server, ::server::Config::default())
                .unwrap();
        let client = AsyncClient::connect(serve_handle.local_addr()).unwrap();

        // Drop future immediately; does the reader channel panic when sending?
        info!("Rpc 1: {}", client.rpc_sync::<_, u64>(&()).unwrap());
        // If the reader panicked, this won't succeed
        info!("Rpc 2: {}", client.rpc_sync::<_, u64>(&()).unwrap());
    }

    #[test]
    fn vec_serialization() {
        let v = vec![1, 2, 3, 4, 5];
        let serialized = super::serialize(&v).unwrap();
        assert_eq!(v, super::deserialize::<Vec<u8>>(&serialized).unwrap());
    }

    #[test]
    fn deregister() {
        use client;
        use server::{self, AsyncServer, AsyncService};
        #[derive(Debug)]
        struct NoopServer;

        impl AsyncService for NoopServer {
            fn handle(&mut self, ctx: GenericCtx, _: Vec<u8>) {
                ctx.reply(Ok(())).unwrap();
            }
        }

        let _ = env_logger::init();
        let server_registry = server::Dispatcher::spawn().unwrap();
        let serve_handle =
            server_registry.register(AsyncServer::new("localhost:0", NoopServer).unwrap())
                .expect(pos!());

        let client_registry = client::Dispatcher::spawn().unwrap();
        let client = client_registry.connect(serve_handle.local_addr()).expect(pos!());
        assert_eq!(client_registry.debug().unwrap().clients, 1);
        let client2 = client.clone();
        assert_eq!(client_registry.debug().unwrap().clients, 1);
        drop(client2);
        assert_eq!(client_registry.debug().unwrap().clients, 1);
        drop(client);
        assert_eq!(client_registry.debug().unwrap().clients, 0);

        assert_eq!(server_registry.debug().unwrap().services, 1);
        let handle2 = serve_handle.clone();
        assert_eq!(server_registry.debug().unwrap().services, 1);
        drop(handle2);
        assert_eq!(server_registry.debug().unwrap().services, 1);
        drop(serve_handle);
        assert_eq!(server_registry.debug().unwrap().services, 0);

    }
}
