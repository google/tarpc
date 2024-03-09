#[tarpc::service(derive_serde = true)]
trait Foo {
    async fn foo();
}

fn main() {
    let x = FooRequest::Foo {};
    x.serialize();
}
