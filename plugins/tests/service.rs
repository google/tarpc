use serde::{Deserialize, Serialize};
use std::hash::Hash;
use tarpc::context;

#[test]
fn att_service_trait() {
    #[tarpc::service]
    trait Foo {
        async fn two_part(s: String, i: i32) -> (String, i32);
        async fn bar(s: String) -> String;
        async fn baz();
    }

    impl Foo for () {
        async fn two_part(self, _: context::Context, s: String, i: i32) -> (String, i32) {
            (s, i)
        }

        async fn bar(self, _: context::Context, s: String) -> String {
            s
        }

        async fn baz(self, _: context::Context) {}
    }
}

#[allow(non_camel_case_types)]
#[test]
fn raw_idents() {
    type r#yield = String;

    #[tarpc::service]
    trait r#trait {
        async fn r#await(r#struct: r#yield, r#enum: i32) -> (r#yield, i32);
        async fn r#fn(r#impl: r#yield) -> r#yield;
        async fn r#async();
    }

    impl r#trait for () {
        async fn r#await(
            self,
            _: context::Context,
            r#struct: r#yield,
            r#enum: i32,
        ) -> (r#yield, i32) {
            (r#struct, r#enum)
        }

        async fn r#fn(self, _: context::Context, r#impl: r#yield) -> r#yield {
            r#impl
        }

        async fn r#async(self, _: context::Context) {}
    }
}

#[test]
fn service_with_cfg_rpc() {
    #[tarpc::service]
    trait Foo {
        async fn foo();
        #[cfg(not(test))]
        async fn bar(s: String) -> String;
    }

    impl Foo for () {
        async fn foo(self, _: context::Context) {}
    }
}

#[test]
fn syntax() {
    #[tarpc::service]
    trait Syntax {
        #[deny(warnings)]
        #[allow(non_snake_case)]
        async fn TestCamelCaseDoesntConflict();
        async fn hello() -> String;
        #[doc = "attr"]
        async fn attr(s: String) -> String;
        async fn no_args_no_return();
        async fn no_args() -> ();
        async fn one_arg(one: String) -> i32;
        async fn two_args_no_return(one: String, two: u64);
        async fn two_args(one: String, two: u64) -> String;
        async fn no_args_ret_error() -> i32;
        async fn one_arg_ret_error(one: String) -> String;
        async fn no_arg_implicit_return_error();
        #[doc = "attr"]
        async fn one_arg_implicit_return_error(one: String);
    }
}

#[test]
fn custom_derives() {
    #[tarpc::service(derive = [Clone, Hash])]
    trait Foo {
        async fn foo();
    }

    fn requires_clone(_: impl Clone) {}
    fn requires_hash(_: impl Hash) {}

    let x = FooRequest::Foo {};
    requires_clone(x.clone());
    requires_hash(x);
}

#[test]
fn implicit_serde() {
    #[tarpc::service]
    trait Foo {
        async fn foo();
    }

    fn requires_serde<T>(_: T)
    where
        for<'de> T: Serialize + Deserialize<'de>,
    {
    }

    let x = FooRequest::Foo {};
    requires_serde(x);
}

#[allow(deprecated)]
#[test]
fn explicit_serde() {
    #[tarpc::service(derive_serde = true)]
    trait Foo {
        async fn foo();
    }

    fn requires_serde<T>(_: T)
    where
        for<'de> T: Serialize + Deserialize<'de>,
    {
    }

    let x = FooRequest::Foo {};
    requires_serde(x);
}
