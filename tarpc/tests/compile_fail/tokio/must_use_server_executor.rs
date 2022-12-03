use futures::stream::once;
use tarpc::{
    context,
    server::{self, incoming::Incoming},
};

#[tarpc::service]
trait World {
    async fn hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

#[tarpc::server]
impl World for HelloServer {
    async fn hello(self, _: context::Context, name: String) -> String {
        format!("Hello, {name}!")
    }
}

fn main() {
    let (_, server_transport) = tarpc::transport::channel::unbounded();
    let server = once(async move { server::BaseChannel::with_defaults(server_transport) });

    #[deny(unused_must_use)]
    {
        server.execute(HelloServer.serve());
    }
}
