use {bincode, future};
use future::server::{Response, Shutdown};
use futures::Future;
use serde::{Deserialize, Serialize};
use std::io;
use std::net::SocketAddr;
use tokio_core::reactor;
use tokio_service::NewService;
#[cfg(feature = "tls")]
use native_tls_inner::TlsAcceptor;

/// Additional options to configure how the server operates.
#[derive(Default)]
pub struct Options {
    opts: future::server::Options,
}

impl Options {
    /// Set the `TlsAcceptor`
    #[cfg(feature = "tls")]
    pub fn tls(mut self, tls_acceptor: TlsAcceptor) -> Self {
        self.opts = self.opts.tls(tls_acceptor);
        self
    }
}

/// A handle to a bound server. Must be run to start serving requests.
#[must_use = "A server does nothing until `run` is called."]
pub struct Handle {
    reactor: reactor::Core,
    handle: future::server::Handle,
    server: Box<Future<Item = (), Error = ()>>,
}

impl Handle {
    #[doc(hidden)]
    pub fn listen<S, Req, Resp, E>(new_service: S,
                                   addr: SocketAddr,
                                   options: Options)
                                   -> io::Result<Self>
        where S: NewService<Request = Result<Req, bincode::Error>,
                            Response = Response<Resp, E>,
                            Error = io::Error> + 'static,
              Req: Deserialize + 'static,
              Resp: Serialize + 'static,
              E: Serialize + 'static
    {
        let reactor = reactor::Core::new()?;
        let (handle, server) =
            future::server::Handle::listen(new_service, addr, &reactor.handle(), options.opts)?;
        let server = Box::new(server);
        Ok(Handle {
            reactor: reactor,
            handle: handle,
            server: server,
        })
    }

    /// Runs the server on the current thread, blocking indefinitely.
    pub fn run(mut self) {
        trace!("Running...");
        match self.reactor.run(self.server) {
            Ok(()) => debug!("Server successfully shutdown."),
            Err(()) => debug!("Server shutdown due to error."),
        }
    }

    /// Returns a hook for shutting down the server.
    pub fn shutdown(&self) -> Shutdown {
        self.handle.shutdown().clone()
    }

    /// The socket address the server is bound to.
    pub fn addr(&self) -> SocketAddr {
        self.handle.addr()
    }
}
