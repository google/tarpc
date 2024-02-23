#[tarpc::service]
trait Foo {
    async fn foo();
}

fn main() {
    let x = FooRequest::Foo {};
    x.serialize();
}
