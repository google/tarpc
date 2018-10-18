#![feature(pin, async_await, await_macro, futures_api, arbitrary_self_types, proc_macro_hygiene)]

use bytes::Bytes;
use log::warn;
use std::{io, collections::HashMap, net::SocketAddr, pin::Pin, sync::{Arc, RwLock}};
use tokio_tcp::{TcpListener, TcpStream};
use futures::{
    future::{ready, FutureObj, Ready},
    compat::{Future01CompatExt, Stream01CompatExt},
    prelude::*,
};
use serde::{Serialize, Deserialize};
use tarpc::{client::{self, Client}, server::{self, Handler}, context};

pub struct ServiceRegistration {
    /// The service's name. Must be unique across all services registered on the ServiceRegistry
    /// this service is registered on
    pub name: String,
    /// The channel over which the service will receive incoming connections.
    pub serve: Box<dyn ServeFn + Send + 'static>,
}

trait ServeFn {
    fn serve(&mut self, cx: context::Context, request: Bytes) -> FutureObj<io::Result<ServiceResponse>>;

    fn box_clone(&self) -> Box<dyn ServeFn + Send + 'static>;
}

impl<Resp, F> ServeFn for F
where
    F: FnMut(context::Context, Bytes) -> Resp + Send + 'static + Clone,
    Resp: Future<Output = io::Result<ServiceResponse>> + Send + 'static
{
    fn serve(&mut self, cx: context::Context, request: Bytes) -> FutureObj<io::Result<ServiceResponse>> {
        FutureObj::new(self(cx, request).boxed())
    }

    fn box_clone(&self) -> Box<dyn ServeFn + Send + 'static> {
        Box::new(self.clone())
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

#[derive(Default)]
pub struct ServiceRegistry {
    registrations: HashMap<String, Box<dyn ServeFn + Sync + Send + 'static>>
}

impl ServiceRegistry {
    /// Spawns a service registry task that listens for incoming connections on the given
    /// TcpListener. Returns a registry for registering new services.
    pub fn spawn(self, listener: TcpListener) {
        let registrations = Arc::new(self.registrations);
        tarpc::Server::new(server::Config::default())
            .incoming(listener.incoming().compat().map(|conn| conn.map(bincode_transport::new)))
            .respond_with(move |cx, req: ServiceRequest| {
                let serve = registrations.get(&req.service_name).map(|serve| serve.box_clone());
                async move {
                    match serve {
                        Some(mut serve) => {
                            await!(serve.serve(cx, req.request))
                        }
                        None => Err(io::Error::new(io::ErrorKind::NotFound,
                                               format!("Service '{}' not registered", req.service_name))),
                    }
                }
            })
            .spawn();
    }

    pub fn register<Req: for<'a> Deserialize<'a> + Send, Resp: Serialize, RespFut: Future<Output=io::Result<Resp>> + Send + 'static>(
        &mut self,
        name: String,
        serve: impl FnMut(context::Context, Req) -> RespFut + Sync + Send + 'static + Clone
    ) {
        self.registrations.insert(name, Box::new(move |cx, req: Bytes| {
            let serve = serve.clone();
            async move {
                let req = bincode::deserialize(req.as_ref()).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                let resp = await!(serve.clone()(cx, req))?;
                let resp = bincode::serialize(&resp).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                Ok(ServiceResponse {
                    response: resp.into(),
                })
            }
        }));
    }

}

pub async fn connect<Req, Resp>(addr: &SocketAddr, service_name: String)
    -> io::Result<Client<Req, Resp>>
    where Req: Serialize + Send,
          Resp: for<'a> Deserialize<'a> + Send
{
    let conn = await!(TcpStream::connect(addr).compat())?;
    let transport = bincode_transport::new::<ServiceResponse, ServiceRequest>(conn)
        .with(|req: Req| async {
            Ok(ServiceRequest {
                service_name: service_name.clone(),
                request: bincode::serialize(&req)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
                    .into(),
            })
        })
        .map_ok(|resp| bincode::deserialize(resp.response.as_ref())
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e)));
    let transport = tarpc::transport::new(transport, transport.peer_addr()?, transport.local_addr()?)
    let client = await!(Client::new(client::Config::default(), transport)?;
    Ok(client)
}

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
    let mut registry = ServiceRegistry::default();
    registry.register("WriteService".to_string(), write_service::serve(server.clone()));
    registry.register("ReadService".to_string(), read_service::serve(server.clone()));

    let listener = TcpListener::bind(&"0.0.0.0:0".parse().unwrap())?;
    let server_addr = listener.local_addr()?;
    registry.spawn(listener);

    Ok(())
}

fn main() {
    tokio::run(
        run()
            .boxed()
            .map_err(|e| panic!(e))
            .boxed()
            .compat(),
    );
}
