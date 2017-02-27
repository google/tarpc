
use future::client::{Client as FutureClient, ClientExt as FutureClientExt,
                     Options as FutureOptions};
/// Exposes a trait for connecting synchronously to servers.
use futures::{self, Future, Stream};
use futures::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
use tokio_core::reactor;
use tokio_service::Service;
use util::FirstSocketAddr;
#[cfg(feature = "tls")]
use tls::client::Context;

#[doc(hidden)]
pub struct Client<Req, Resp, E> {
    request: UnboundedSender<(Req, mpsc::Sender<Result<Resp, ::Error<E>>>)>,
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

impl Into<FutureOptions> for (reactor::Handle, Options) {
    #[cfg(feature = "tls")]
    fn into(self) -> FutureOptions {
        let (handle, options) = self;
        let mut opts = FutureOptions::default().handle(handle);
        if let Some(tls_ctx) = options.tls_ctx {
            opts = opts.tls(tls_ctx);
        }
        opts
    }

    #[cfg(not(feature = "tls"))]
    fn into(self) -> FutureOptions {
        let (handle, _) = self;
        FutureOptions::default().handle(handle)
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
    fn connect<A>(addr: A, options: Options) -> io::Result<Self>
        where A: ToSocketAddrs
    {
        let addr = addr.try_first_socket_addr()?;
        let (connect_tx, connect_rx) = mpsc::channel();
        thread::spawn(move || {
            match RequestHandler::connect(addr, options) {
                Ok((mut handler, request)) => {
                    connect_tx.send(Ok(request)).unwrap();
                    handler.handle_requests();
                }
                Err(e) => connect_tx.send(Err(e)).unwrap(),
            }
        });
        Ok(Client { request: connect_rx.recv().unwrap()? })
    }
}

/// Forwards incoming requests of type `Req`
/// with expected response `Result<Resp, ::Error<E>>`
/// to service `S`.
struct RequestHandler<Req, Resp, E, S> {
    reactor: reactor::Core,
    client: S,
    requests: UnboundedReceiver<(Req, mpsc::Sender<Result<Resp, ::Error<E>>>)>,
}

impl<Req, Resp, E> RequestHandler<Req, Resp, E, FutureClient<Req, Resp, E>>
    where Req: Serialize + Sync + Send + 'static,
          Resp: Deserialize + Sync + Send + 'static,
          E: Deserialize + Sync + Send + 'static
{
    /// Creates a new `RequestHandler` by connecting a `FutureClient` to the given address
    /// using the given options.
    fn connect(addr: SocketAddr, options: Options)
        -> io::Result<(Self, UnboundedSender<(Req, mpsc::Sender<Result<Resp, ::Error<E>>>)>)>
    {
        let mut reactor = reactor::Core::new()?;
        let options = (reactor.handle(), options).into();
        let client = reactor.run(FutureClient::connect(addr, options))?;
        let (request, requests) = futures::sync::mpsc::unbounded();
        Ok((RequestHandler {reactor, client, requests }, request))
    }
}

impl<Req, Resp, E, S> RequestHandler<Req, Resp, E, S>
    where Req: Serialize + 'static,
          Resp: Deserialize + 'static,
          E: Deserialize + 'static,
          S: Service<Request = Req, Response = Resp, Error = ::Error<E>>,
          S::Future: 'static,
{
    fn handle_requests(&mut self) {
        let RequestHandler { ref mut reactor, ref mut requests, ref mut client } = *self;
        let handle = reactor.handle();
        let requests = requests.for_each(|(request, response_tx)| {
            let request = client.call(request)
                .then(move |response| {
                    response_tx.send(response).unwrap();
                    Ok(())
                  });
            handle.spawn(request);
            Ok(())
        });
        reactor.run(requests).unwrap();
    }
}

#[test]
fn handle_requests() {
    extern crate service_fn;
    let (request, requests) = futures::sync::mpsc::unbounded();
    let reactor = reactor::Core::new().unwrap();
    let client = service_fn::service_fn(|i: i32| -> Result<i32, ::Error<()>> { Ok(i) });
    let mut request_handler = RequestHandler { reactor, client, requests };
    // Test that `handle_requests` returns when all request senders are dropped.
    drop(request);
    request_handler.handle_requests();
}
