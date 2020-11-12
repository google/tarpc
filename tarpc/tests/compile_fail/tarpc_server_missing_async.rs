#[tarpc::service(derive_serde = false)]
trait World {
    async fn hello(name: String) -> String;
}

struct HelloServer;

#[tarpc::server]
impl World for HelloServer {
    fn hello(name: String) ->  String {
        format!("Hello, {}!", name)
    }
}

fn main() {}
