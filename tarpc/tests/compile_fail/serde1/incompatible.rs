#![allow(deprecated)]
#[tarpc::service(derive = [Clone], derive_serde = true)]
trait Foo {
    async fn foo();
}

fn main() {}
