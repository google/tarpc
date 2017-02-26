
use future::client::{Client as FutureClient, ClientExt as FutureClientExt, Options as FutureOptions};
/// Exposes a trait for connecting synchronously to servers.
use futures::{self, Future, Stream};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;
use std::net::ToSocketAddrs;
use std::sync::mpsc;
use std::thread;
use tokio_core::reactor;
use tokio_service::Service;
use util::FirstSocketAddr;
#[cfg(feature = "tls")]
use tls::client::Context;

#[doc(hidden)]
pub struct Client<Req, Resp, E> {
    request: futures::sync::mpsc::UnboundedSender<(Req, mpsc::Sender<Result<Resp, ::Error<E>>>)>,
}

impl<Req, Resp, E> Clone for Client<Req, Resp, E> {
    fn clone(&self) -> Self {
        Client { request: self.request.clone() }
    }
}

impl<Req, Resp, E> fmt::Debug for Client<Req, Resp, E> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "Client {{ .. }}")
    }
}

impl<Req, Resp, E> Client<Req, Resp, E>
    where Req: Serialize + Sync + Send + 'static,
          Resp: Deserialize + Sync + Send + 'static,
          E: Deserialize + Sync + Send + 'static
{
    /// Drives an RPC call for the given request.
    pub fn call(&self, request: Req) -> Result<Resp, ::Error<E>> {
        let (tx, rx) = mpsc::channel();
        self.request.send((request, tx)).unwrap();
        rx.recv().unwrap()
    }
}

/// Additional options to configure how the client connects and operates.
#[derive(Default)]
pub struct Options {
    #[cfg(feature = "tls")]
    tls_ctx: Option<Context>,
}

impl Options {
    /// Connect using the given `Context`
    #[cfg(feature = "tls")]
    pub fn tls(mut self, ctx: Context) -> Self {
        self.tls_ctx = Some(ctx);
        self
    }
}

/// Extension methods for Clients.
pub trait ClientExt: Sized {
    /// Connects to a server located at the given address.
    fn connect<A>(addr: A, options: Options) -> io::Result<Self> where A: ToSocketAddrs;
}

impl<Req, Resp, E> ClientExt for Client<Req, Resp, E>
    where Req: Serialize + Sync + Send + 'static,
          Resp: Deserialize + Sync + Send + 'static,
          E: Deserialize + Sync + Send + 'static
{
    fn connect<A>(addr: A, _options: Options) -> io::Result<Self>
        where A: ToSocketAddrs
    {
        let addr = addr.try_first_socket_addr()?;
        let (connect_tx, connect_rx) = mpsc::channel();
        let (request, request_rx) = futures::sync::mpsc::unbounded();
        #[cfg(feature = "tls")]
        let tls_ctx = _options.tls_ctx;
        thread::spawn(move || {
            let mut reactor = match reactor::Core::new() {
                Ok(reactor) => reactor,
                Err(e) => {
                    connect_tx.send(Err(e)).unwrap();
                    return;
                }
            };
            let options;
            #[cfg(feature = "tls")]
            {
                let mut opts = FutureOptions::default().handle(reactor.handle());
                if let Some(tls_ctx) = tls_ctx {
                    opts = opts.tls(tls_ctx);
                }
                options = opts;
            }
            #[cfg(not(feature = "tls"))]
            {
                options = FutureOptions::default().handle(reactor.handle());
            }
            let client = match reactor.run(FutureClient::connect(addr, options)) {
                Ok(client) => {
                    connect_tx.send(Ok(())).unwrap();
                    client
                }
                Err(e) => {
                    connect_tx.send(Err(e)).unwrap();
                    return;
                }
            };
            let handle = reactor.handle();
            let requests = request_rx.for_each(|(request, response_tx): (_, mpsc::Sender<_>)| {
                handle.spawn(client.call(request)
                    .then(move |response| Ok(response_tx.send(response).unwrap())));
                Ok(())
            });
            reactor.run(requests).unwrap();
        });
        connect_rx.recv().unwrap()?;
        Ok(Client { request: request })
    }
}
