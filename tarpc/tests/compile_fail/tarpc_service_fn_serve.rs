#[tarpc::service]
trait World {
    async fn serve();
}

fn main() {}
