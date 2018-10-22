#![feature(pin, async_await, await_macro, futures_api, arbitrary_self_types, proc_macro_hygiene, impl_trait_in_bindings)]

mod registry {
    use bytes::Bytes;
    use std::{io, net::SocketAddr, pin::Pin, sync::Arc, task::{LocalWaker, Poll}};
    use tokio_tcp::{TcpStream, TcpListener};
    use futures::{
        future::ready,
        compat::{Future01CompatExt, Stream01CompatExt},
        prelude::*,
    };
    use serde::{Serialize, Deserialize};
    use tarpc::{client::{self, Client}, server::Handler, context};

    pub struct Cons<Car, Cdr>(pub Car, pub Cdr);
    pub struct Nil;

    pub trait Serve: Clone + Send + 'static {
        type Response: Future<Output = io::Result<ServiceResponse>> + Send + 'static;
        fn serve(self, cx: context::Context, request: Bytes) -> Self::Response;
    }

    pub trait ServiceList: Send + 'static {
        type Future: Future<Output = io::Result<ServiceResponse>> + Send + 'static;

        fn serve(&self, cx: context::Context, request: &ServiceRequest) -> Option<Self::Future>;
    }

    impl ServiceList for Nil {
        type Future = futures::future::Ready<io::Result<ServiceResponse>>;

        fn serve(&self, _: context::Context, _: &ServiceRequest) -> Option<Self::Future> {
            None
        }
    }

    impl<Car, Cdr> ServiceList for Cons<Car, Cdr>
    where
        Car: ServiceList,
        Cdr: ServiceList,
    {
        type Future = Either<Car::Future, Cdr::Future>;

        fn serve(&self, cx: context::Context, request: &ServiceRequest) -> Option<Self::Future> {
            match self.0.serve(cx, request) {
                Some(response) => Some(Either::Left(response)),
                None => self.1.serve(cx, request).map(Either::Right),
            }
        }
    }

    pub enum Either<Left, Right> {
        Left(Left),
        Right(Right),
    }

    impl<Output, Left, Right> Future for Either<Left, Right>
    where
        Left: Future<Output = Output>,
        Right: Future<Output = Output>
    {
        type Output = Output;

        fn poll(self: Pin<&mut Self>, waker: &LocalWaker) -> Poll<Output> {
            unsafe {
                match Pin::get_mut_unchecked(self) {
                    Either::Left(car) => Pin::new_unchecked(car).poll(waker),
                    Either::Right(cdr) => Pin::new_unchecked(cdr).poll(waker),
                }
            }
        }
    }

    pub struct ServiceRegistration<S: Serve> {
        /// The service's name. Must be unique across all services registered on the ServiceRegistry
        /// this service is registered on
        pub name: String,
        /// The channel over which the service will receive incoming connections.
        pub serve: S,
    }

    impl<S: Serve> ServiceList for ServiceRegistration<S> {
        type Future = S::Response;
        fn serve(&self, cx: context::Context, request: &ServiceRequest) -> Option<Self::Future> {
            if self.name == request.service_name {
                Some(self.serve.clone().serve(cx, request.request.clone()))
            } else {
                None
            }
        }
    }

    impl<Resp, F> Serve for F
    where
        F: FnOnce(context::Context, Bytes) -> Resp + Clone + Send + 'static,
        Resp: Future<Output = io::Result<ServiceResponse>> + Send + 'static
    {
        type Response = Resp;

        fn serve(self, cx: context::Context, request: Bytes) -> Resp {
            self(cx, request)
        }
    }

    #[derive(Serialize, Deserialize)]
    pub struct ServiceRequest {
        service_name: String,
        request: Bytes,
    }

    #[derive(Serialize, Deserialize)]
    pub struct ServiceResponse {
        response: Bytes,
    }

    pub type Registration<S, Rest> = Cons<ServiceRegistration<S>, Rest>;

    pub struct ServiceRegistry<Services> {
        registrations: Services,
    }

    impl Default for ServiceRegistry<Nil> {
        fn default() -> Self {
            ServiceRegistry { registrations: Nil }
        }
    }

    impl<Services: ServiceList + Sync> ServiceRegistry<Services> {
        /// Spawns a service registry task that listens for incoming connections on the given
        /// TcpListener. Returns a registry for registering new services.
        pub fn spawn(self, listener: TcpListener) {
            let registrations = Arc::new(self.registrations);
            let server = tarpc::Server::default()
                .incoming(listener.incoming().compat().map(|conn| conn.map(bincode_transport::new)))
                .take(1)
                .respond_with(move |cx, req: ServiceRequest| {
                    match registrations.serve(cx, &req) {
                        Some(serve) => {
                            Either::Left(serve)
                        }
                        None => Either::Right(ready(Err(io::Error::new(io::ErrorKind::NotFound,
                                               format!("Service '{}' not registered", req.service_name))))),
                    }
                });
            tokio_executor::spawn(server.unit_error().boxed().compat());
        }

        pub fn register<S, Req, Resp, RespFut>(self, name: String, serve: S) -> ServiceRegistry<Registration<impl Serve + Send + 'static, Services>>
        where
            Req: for<'a> Deserialize<'a> + Send,
            Resp: Serialize,
            S: FnMut(context::Context, Req) -> RespFut + Send + 'static + Clone,
            RespFut: Future<Output=io::Result<Resp>> + Send + 'static,
        {
            let registration = ServiceRegistration {
                name: name,
                serve: move |cx, req: Bytes| {
                    async move {
                        let req = bincode::deserialize(req.as_ref()).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                        let resp = await!(serve.clone()(cx, req))?;
                        let resp = bincode::serialize(&resp).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                        Ok(ServiceResponse {
                            response: resp.into(),
                        })
                    }
                }
            };
            ServiceRegistry { registrations: Cons(registration, self.registrations) }
        }
    }

    pub async fn connect(addr: &SocketAddr)
        -> io::Result<client::Channel<ServiceRequest, ServiceResponse>>
    {
        let conn = await!(TcpStream::connect(addr).compat())?;
        let transport = bincode_transport::new(conn);
        let channel = await!(client::new(client::Config::default(), transport))?;
        Ok(channel)
    }

    pub fn new_client<Req, Resp>(service_name: String, channel: &client::Channel<ServiceRequest, ServiceResponse>)
        -> client::MapResponse<client::WithRequest<client::Channel<ServiceRequest, ServiceResponse>, impl FnMut(Req) -> ServiceRequest>, impl FnMut(ServiceResponse) -> Resp>
    where Req: Serialize + Send + 'static,
          Resp: for<'a> Deserialize<'a> + Send + 'static,
    {
        channel.clone()
            .with_request(move |req| {
                ServiceRequest {
                    service_name: service_name.clone(),
                    request: bincode::serialize(&req).unwrap().into(),
                }
            })
            .map_response(|resp| {
                bincode::deserialize(resp.response.as_ref()).unwrap()
            })
    }
}

// Example
use std::{io, collections::HashMap, sync::{Arc, RwLock}};
use tokio_tcp::TcpListener;
use futures::{
    future::{ready, Ready},
    prelude::*,
};
use serde::{Serialize, Deserialize};
use tarpc::{context};

mod write_service {
    tarpc::service! {
        rpc write(key: String, value: String);
    }
}

mod read_service {
    tarpc::service! {
        rpc read(key: String) -> Option<String>;
    }
}

#[derive(Debug, Default, Clone)]
struct Server {
    data: Arc<RwLock<HashMap<String, String>>>,
}

impl write_service::Service for Server {
    type WriteFut = Ready<()>;

    fn write(&self, _: context::Context, key: String, value: String) -> Self::WriteFut {
        self.data.write().unwrap().insert(key, value);
        ready(())
    }
}

impl read_service::Service for Server {
    type ReadFut = Ready<Option<String>>;

    fn read(&self, _: context::Context, key: String) -> Self::ReadFut {
        ready(self.data.read().unwrap().get(&key).cloned())
    }
}

trait DefaultSpawn {
    fn spawn(self);
}

impl<F> DefaultSpawn for F
    where F: Future<Output = ()> + Send + 'static
{
    fn spawn(self) {
        tokio_executor::spawn(self.unit_error().boxed().compat())
    }
}

async fn run() -> io::Result<()> {
    let server = Server::default();
    let registry = registry::ServiceRegistry::default()
        .register("WriteService".to_string(), write_service::serve(server.clone()))
        .register("ReadService".to_string(), read_service::serve(server.clone()));

    let listener = TcpListener::bind(&"0.0.0.0:0".parse().unwrap())?;
    let server_addr = listener.local_addr()?;
    registry.spawn(listener);

    let channel = await!(registry::connect(&server_addr))?;

    let write_client = registry::new_client("WriteService".to_string(), &channel);
    let mut write_client = write_service::Client::from(write_client);

    let read_client = registry::new_client("ReadService".to_string(), &channel);
    let mut read_client = read_service::Client::from(read_client);

    await!(write_client.write(context::current(), "key".to_string(), "val".to_string()))?;
    let val = await!(read_client.read(context::current(), "key".to_string()))?;
    println!("{:?}", val);

    Ok(())
}

fn main() {
    tarpc::init(futures::compat::TokioDefaultSpawner);
    tokio::run(
        run()
            .boxed()
            .map_err(|e| panic!(e))
            .boxed()
            .compat(),
    );
}
