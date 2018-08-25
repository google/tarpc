use crate::future::server::{Response, Shutdown};
use futures::sync::oneshot;
use futures::{future as futures, Future};
#[cfg(feature = "tls")]
use native_tls_inner::TlsAcceptor;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;
use std::usize;
use thread_pool::{self, Sender, Task, ThreadPool};
use tokio_core::reactor;
use tokio_service::{NewService, Service};
use crate::{bincode, future, num_cpus};

/// Additional options to configure how the server operates.
#[derive(Debug)]
pub struct Options {
    thread_pool: thread_pool::Builder,
    opts: future::server::Options,
}

impl Default for Options {
    fn default() -> Self {
        let num_cpus = num_cpus::get();
        Options {
            thread_pool: thread_pool::Builder::new()
                .keep_alive(Duration::from_secs(60))
                .max_pool_size(num_cpus * 100)
                .core_pool_size(num_cpus)
                .work_queue_capacity(usize::MAX)
                .name_prefix("request-thread-"),
            opts: future::server::Options::default(),
        }
    }
}

impl Options {
    /// Set the max payload size in bytes. The default is 2,000,000 (2 MB).
    pub fn max_payload_size(mut self, bytes: u64) -> Self {
        self.opts = self.opts.max_payload_size(bytes);
        self
    }

    /// Sets the thread pool builder to use when creating the server's thread pool.
    pub fn thread_pool(mut self, builder: thread_pool::Builder) -> Self {
        self.thread_pool = builder;
        self
    }

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

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        const SERVER: &str = "Box<Future<Item = (), Error = ()>>";

        f.debug_struct("Handle")
            .field("reactor", &self.reactor)
            .field("handle", &self.handle)
            .field("server", &SERVER)
            .finish()
    }
}

#[doc(hidden)]
pub fn listen<S, Req, Resp, E>(
    new_service: S,
    addr: SocketAddr,
    options: Options,
) -> io::Result<Handle>
where
    S: NewService<
            Request = Result<Req, bincode::Error>,
            Response = Response<Resp>,
            Error = io::Error,
        > + 'static,
    <S::Instance as Service>::Future: Send + 'static,
    S::Response: Send,
    S::Error: Send,
    Req: DeserializeOwned + 'static,
    Resp: Serialize + 'static,
    E: Serialize + 'static,
{
    let new_service = NewThreadService::new(new_service, options.thread_pool);
    let reactor = reactor::Core::new()?;
    let (handle, server) =
        future::server::listen(new_service, addr, &reactor.handle(), options.opts)?;
    let server = Box::new(server);
    Ok(Handle {
        reactor: reactor,
        handle: handle,
        server: server,
    })
}

/// A service that uses a thread pool.
struct NewThreadService<S>
where
    S: NewService,
{
    new_service: S,
    sender: Sender<ServiceTask<<S::Instance as Service>::Future>>,
    _pool: ThreadPool<ServiceTask<<S::Instance as Service>::Future>>,
}

/// A service that runs by executing request handlers in a thread pool.
struct ThreadService<S>
where
    S: Service,
{
    service: S,
    sender: Sender<ServiceTask<S::Future>>,
}

/// A task that handles a single request.
struct ServiceTask<F>
where
    F: Future,
{
    future: F,
    tx: oneshot::Sender<Result<F::Item, F::Error>>,
}

impl<S> NewThreadService<S>
where
    S: NewService,
    <S::Instance as Service>::Future: Send + 'static,
    S::Response: Send,
    S::Error: Send,
{
    /// Create a NewThreadService by wrapping another service.
    fn new(new_service: S, pool: thread_pool::Builder) -> Self {
        let (sender, _pool) = pool.build();
        NewThreadService {
            new_service,
            sender,
            _pool,
        }
    }
}

impl<S> NewService for NewThreadService<S>
where
    S: NewService,
    <S::Instance as Service>::Future: Send + 'static,
    S::Response: Send,
    S::Error: Send,
{
    type Request = S::Request;
    type Response = S::Response;
    type Error = S::Error;
    type Instance = ThreadService<S::Instance>;

    fn new_service(&self) -> io::Result<Self::Instance> {
        Ok(ThreadService {
            service: self.new_service.new_service()?,
            sender: self.sender.clone(),
        })
    }
}

impl<F> Task for ServiceTask<F>
where
    F: Future + Send + 'static,
    F::Item: Send,
    F::Error: Send,
{
    fn run(self) {
        // Don't care if sending fails. It just means the request is no longer
        // being handled (I think).
        let _ = self.tx.send(self.future.wait());
    }
}

impl<S> Service for ThreadService<S>
where
    S: Service,
    S::Future: Send + 'static,
    S::Response: Send,
    S::Error: Send,
{
    type Request = S::Request;
    type Response = S::Response;
    type Error = S::Error;
    type Future = futures::AndThen<
        futures::MapErr<
            oneshot::Receiver<Result<Self::Response, Self::Error>>,
            fn(oneshot::Canceled) -> Self::Error,
        >,
        Result<Self::Response, Self::Error>,
        fn(Result<Self::Response, Self::Error>) -> Result<Self::Response, Self::Error>,
    >;

    fn call(&self, request: Self::Request) -> Self::Future {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(ServiceTask {
                future: self.service.call(request),
                tx: tx,
            }).unwrap();
        rx.map_err(unreachable as _).and_then(ident)
    }
}

fn unreachable<T, U>(t: T) -> U
where
    T: fmt::Display,
{
    unreachable!(t)
}

fn ident<T>(t: T) -> T {
    t
}
