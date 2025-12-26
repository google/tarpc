use tarpc::client;
use tarpc::context::DefaultContext;
#[tarpc::service]
trait World {
    async fn hello(name: String) -> String;
}

fn main() {
    let (client_transport, _) = tarpc::transport::channel::unbounded();

    #[deny(unused_must_use)]
    {
        WorldClient::<DefaultContext>::new(client::Config::default(), client_transport).dispatch;
    }
}
