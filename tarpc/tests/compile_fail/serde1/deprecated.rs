#![deny(warnings)]

#[tarpc::service(derive_serde = true)]
trait Foo {
    async fn foo();
}

fn main() {}
