use future::client::{Client as FutureClient, ClientExt as FutureClientExt,
                     Options as FutureOptions};
use futures::{Future, Stream};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
#[cfg(feature = "tls")]
use tls::client::Context;
use tokio_core::reactor;
use tokio_proto::util::client_proxy::{ClientProxy, Receiver, pair};
use tokio_service::Service;
use util::FirstSocketAddr;

#[doc(hidden)]
pub struct Client<Req, Resp, E> {
    proxy: ClientProxy<Req, Resp, ::Error<E>>,
}

impl<Req, Resp, E> Clone for Client<Req, Resp, E> {
    fn clone(&self) -> Self {
        Client {
            proxy: self.proxy.clone(),
        }
    }
}

impl<Req, Resp, E> fmt::Debug for Client<Req, Resp, E> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        const PROXY: &str = "ClientProxy { .. }";
        f.debug_struct("Client").field("proxy", &PROXY).finish()
    }
}

impl<Req, Resp, E> Client<Req, Resp, E>
where
    Req: Serialize + Send + 'static,
    Resp: DeserializeOwned + Send + 'static,
    E: DeserializeOwned + Send + 'static,
{
    /// Drives an RPC call for the given request.
    pub fn call(&self, request: Req) -> Result<Resp, ::Error<E>> {
        // Must call wait here to block on the response.
        // The request handler relies on this fact to safely unwrap the
        // oneshot send.
        self.proxy.call(request).wait()
    }
}

/// Additional options to configure how the client connects and operates.
pub struct Options {
    /// Max packet size in bytes.
    max_payload_size: u64,
    #[cfg(feature = "tls")]
    tls_ctx: Option<Context>,
}

impl Default for Options {
    #[cfg(not(feature = "tls"))]
    fn default() -> Self {
        Options {
            max_payload_size: 2_000_000,
        }
    }

    #[cfg(feature = "tls")]
    fn default() -> Self {
        Options {
            max_payload_size: 2_000_000,
            tls_ctx: None,
        }
    }
}

impl Options {
    /// Set the max payload size in bytes. The default is 2,000,000 (2 MB).
    pub fn max_payload_size(mut self, bytes: u64) -> Self {
        self.max_payload_size = bytes;
        self
    }

    /// Connect using the given `Context`
    #[cfg(feature = "tls")]
    pub fn tls(mut self, ctx: Context) -> Self {
        self.tls_ctx = Some(ctx);
        self
    }
}

impl fmt::Debug for Options {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        #[cfg(feature = "tls")]
        const SOME: &str = "Some(_)";
        #[cfg(feature = "tls")]
        const NONE: &str = "None";
        let mut f = f.debug_struct("Options");
        #[cfg(feature = "tls")] f.field("tls_ctx", if self.tls_ctx.is_some() { &SOME } else { &NONE });
        f.finish()
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
    fn connect<A>(addr: A, options: Options) -> io::Result<Self>
    where
        A: ToSocketAddrs;
}

impl<Req, Resp, E> ClientExt for Client<Req, Resp, E>
where
    Req: Serialize + Send + 'static,
    Resp: DeserializeOwned + Send + 'static,
    E: DeserializeOwned + Send + 'static,
{
    fn connect<A>(addr: A, options: Options) -> io::Result<Self>
    where
        A: ToSocketAddrs,
    {
        let addr = addr.try_first_socket_addr()?;
        let (connect_tx, connect_rx) = mpsc::channel();
        thread::spawn(move || match RequestHandler::connect(addr, options) {
            Ok((proxy, mut handler)) => {
                connect_tx.send(Ok(proxy)).unwrap();
                handler.handle_requests();
            }
            Err(e) => connect_tx.send(Err(e)).unwrap(),
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
where
    Req: Serialize + Send + 'static,
    Resp: DeserializeOwned + Send + 'static,
    E: DeserializeOwned + Send + 'static,
{
    /// Creates a new `RequestHandler` by connecting a `FutureClient` to the given address
    /// using the given options.
    fn connect(addr: SocketAddr, options: Options) -> io::Result<(Client<Req, Resp, E>, Self)> {
        let mut reactor = reactor::Core::new()?;
        let options = (reactor.handle(), options).into();
        let client = reactor.run(FutureClient::connect(addr, options))?;
        let (proxy, requests) = pair();
        Ok((
            Client { proxy },
            RequestHandler {
                reactor,
                client,
                requests,
            },
        ))
    }
}

impl<Req, Resp, E, S> RequestHandler<Req, Resp, E, S>
where
    Req: Serialize + 'static,
    Resp: DeserializeOwned + 'static,
    E: DeserializeOwned + 'static,
    S: Service<Request = Req, Response = Resp, Error = ::Error<E>>,
    S::Future: 'static,
{
    fn handle_requests(&mut self) {
        let RequestHandler {
            ref mut reactor,
            ref mut requests,
            ref mut client,
        } = *self;
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
                let request = client.call(request).then(move |response| {
                    // Safe to unwrap because clients always block on the response future.
                    response_tx
                        .send(response)
                        .map_err(|_| ())
                        .expect("Client should block on response");
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
    let mut request_handler = RequestHandler {
        reactor,
        client,
        requests,
    };
    // Test that `handle_requests` returns when all request senders are dropped.
    drop(request);
    request_handler.handle_requests();
}
