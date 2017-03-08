
use future::client::{Client as FutureClient, ClientExt as FutureClientExt,
                     Options as FutureOptions};
/// Exposes a trait for connecting synchronously to servers.
use futures::{Future, Stream};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
use tokio_core::reactor;
use tokio_proto::util::client_proxy::{ClientProxy, Receiver, pair};
use tokio_service::Service;
use util::FirstSocketAddr;
#[cfg(feature = "tls")]
use tls::client::Context;

#[doc(hidden)]
pub struct Client<Req, Resp, E> {
    proxy: ClientProxy<Req, Resp, ::Error<E>>,
}

impl<Req, Resp, E> Clone for Client<Req, Resp, E> {
    fn clone(&self) -> Self {
        Client { proxy: self.proxy.clone() }
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
        self.proxy.call(request).wait()
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
                Ok((proxy, mut handler)) => {
                    connect_tx.send(Ok(proxy)).unwrap();
                    handler.handle_requests();
                }
                Err(e) => connect_tx.send(Err(e)).unwrap(),
            }
        });
        Ok(connect_rx.recv().unwrap()?)
    }
}

/// Forwards incoming requests of type `Req`
/// with expected response `Result<Resp, ::Error<E>>`
/// to service `S`.
struct RequestHandler<Req, Resp, E, S> {
    reactor: reactor::Core,
    client: S,
    requests: Receiver<Req, Resp, ::Error<E>>,
}

impl<Req, Resp, E> RequestHandler<Req, Resp, E, FutureClient<Req, Resp, E>>
    where Req: Serialize + Sync + Send + 'static,
          Resp: Deserialize + Sync + Send + 'static,
          E: Deserialize + Sync + Send + 'static
{
    /// Creates a new `RequestHandler` by connecting a `FutureClient` to the given address
    /// using the given options.
    fn connect(addr: SocketAddr, options: Options)
        -> io::Result<(Client<Req, Resp, E>, Self)>
    {
        let mut reactor = reactor::Core::new()?;
        let options = (reactor.handle(), options).into();
        let client = reactor.run(FutureClient::connect(addr, options))?;
        let (proxy, requests) = pair();
        Ok((Client { proxy }, RequestHandler { reactor, client, requests }))
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
        let requests = requests
            .map(|result| {
                match result {
                    Ok(req) => req,
                    // The ClientProxy never sends Err currently
                    Err(e) => panic!("Unimplemented error handling in RequestHandler: {}", e),
                }
            })
            .for_each(|(request, response_tx)| {
                let request = client.call(request)
                    .then(move |response| {
                        response_tx.complete(response);
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
    use futures::future;

    struct Client;
    impl Service for Client {
        type Request = i32;
        type Response = i32;
        type Error = ::Error<()>;
        type Future = future::FutureResult<i32, ::Error<()>>;

        fn call(&self, req: i32) -> Self::Future {
            future::ok(req)
        }
    }

    let (request, requests) = ::futures::sync::mpsc::unbounded();
    let reactor = reactor::Core::new().unwrap();
    let client = Client;
    let mut request_handler = RequestHandler { reactor, client, requests };
    // Test that `handle_requests` returns when all request senders are dropped.
    drop(request);
    request_handler.handle_requests();
}
