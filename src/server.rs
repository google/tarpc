// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use bincode;
use errors::WireError;
use futures::{Future, Stream, future};
use net2;
use protocol::Proto;
use serde::{Deserialize, Serialize};
use std::io;
use std::net::SocketAddr;
use tokio_core::net::TcpListener;
use tokio_core::reactor;
use tokio_proto::BindServer;
use tokio_service::NewService;

cfg_if! {
    if #[cfg(feature = "tls")] {
        use native_tls::TlsAcceptor;
        use tokio_tls::TlsAcceptorExt;
        use errors::native_to_io;
        use stream_type::StreamType;
    } else {}
}

enum Acceptor {
    Tcp,
    #[cfg(feature = "tls")]
    Tls(TlsAcceptor),
}

/// Additional options to configure how the server operates.
#[derive(Default)]
pub struct Options {
    #[cfg(feature = "tls")]
    tls_acceptor: Option<TlsAcceptor>,
}

impl Options {
    /// Set the `TlsAcceptor`
    #[cfg(feature = "tls")]
    pub fn tls(mut self, tls_acceptor: TlsAcceptor) -> Self {
        self.tls_acceptor = Some(tls_acceptor);
        self
    }
}

/// A message from server to client.
#[doc(hidden)]
pub type Response<T, E> = Result<T, WireError<E>>;

#[doc(hidden)]
pub fn listen<S, Req, Resp, E>(new_service: S,
                               addr: SocketAddr,
                               handle: &reactor::Handle,
                               _options: Options)
                               -> io::Result<(SocketAddr, impl Future<Item = (), Error = ()>)>
    where S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: Deserialize + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    // Similar to the client, since `Options` is not `Send`, we take the `TlsAcceptor` when it is
    // available.
    #[cfg(feature = "tls")]
    let acceptor = match _options.tls_acceptor {
        Some(tls_acceptor) => Acceptor::Tls(tls_acceptor),
        None => Acceptor::Tcp,
    };
    #[cfg(not(feature = "tls"))]
    let acceptor = Acceptor::Tcp;

    listen_with(new_service, addr, handle, acceptor)
}

/// A handle to a bound server. Must be run to start serving requests.
pub struct Handle {
    reactor: reactor::Core,
    addr: SocketAddr,
}

impl Handle {
    #[doc(hidden)]
    pub fn new(reactor: reactor::Core, addr: SocketAddr) -> Self {
        Handle {
            reactor: reactor,
            addr: addr,
        }
    }

    /// Runs the server on the current thread, blocking indefinitely.
    pub fn run(&mut self) -> ! {
        loop {
            self.reactor.turn(None)
        }
    }

    /// The socket address the server is bound to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

/// Spawns a service that binds to the given address using the given handle.
fn listen_with<S, Req, Resp, E>(new_service: S,
                                addr: SocketAddr,
                                handle: &reactor::Handle,
                                _acceptor: Acceptor)
                                -> io::Result<(SocketAddr, impl Future<Item = (), Error = ()>)>
    where S: NewService<Request = Result<Req, bincode::Error>,
                        Response = Response<Resp, E>,
                        Error = io::Error> + 'static,
          Req: Deserialize + 'static,
          Resp: Serialize + 'static,
          E: Serialize + 'static
{
    let listener = listener(&addr, handle)?;
    let addr = listener.local_addr()?;

    let handle2 = handle.clone();

    let server = listener.incoming()
        .and_then(move |(socket, _)| {
            #[cfg(feature = "tls")]
            match _acceptor {
                Acceptor::Tls(ref tls_acceptor) => {
                    future::Either::A(tls_acceptor.accept_async(socket)
                        .map(StreamType::Tls)
                        .map_err(native_to_io))
                }
                Acceptor::Tcp => future::Either::B(future::ok(StreamType::Tcp(socket))),
            }
            #[cfg(not(feature = "tls"))]
            future::ok(socket)
        })
        .for_each(move |socket| {
            Proto::new().bind_server(&handle2, socket, new_service.new_service()?);

            Ok(())
        })
        .map_err(|e| error!("While processing incoming connections: {}", e));
    Ok((addr, server))
}

fn listener(addr: &SocketAddr, handle: &reactor::Handle) -> io::Result<TcpListener> {
    const PENDING_CONNECTION_BACKLOG: i32 = 1024;
    #[cfg(unix)]
    use net2::unix::UnixTcpBuilderExt;

    let builder = match *addr {
        SocketAddr::V4(_) => net2::TcpBuilder::new_v4(),
        SocketAddr::V6(_) => net2::TcpBuilder::new_v6(),
    }?;

    builder.reuse_address(true)?;

    #[cfg(unix)]
    builder.reuse_port(true)?;

    builder.bind(addr)?
        .listen(PENDING_CONNECTION_BACKLOG)
        .and_then(|l| TcpListener::from_listener(l, addr, handle))
}
