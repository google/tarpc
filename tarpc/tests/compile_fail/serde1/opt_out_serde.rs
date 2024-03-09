#![allow(deprecated)]

use std::fmt::Formatter;

#[tarpc::service(derive_serde = false)]
trait Foo {
    async fn foo();
}

fn foo(f: &mut Formatter) {
    let x = FooRequest::Foo {};
    tarpc::serde::Serialize::serialize(&x, f);
}

fn main() {}
