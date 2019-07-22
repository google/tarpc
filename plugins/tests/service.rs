#![feature(async_await)]

use tarpc::context;

#[test]
fn att_service_trait() {
    use futures::future::{ready, Ready};

    #[tarpc::service]
    trait Foo {
        async fn two_part(s: String, i: i32) -> (String, i32);
        async fn bar(s: String) -> String;
        async fn baz();
    }

    impl Foo for () {
        type TwoPartFut = Ready<(String, i32)>;
        fn two_part(self, _: context::Context, s: String, i: i32) -> Self::TwoPartFut {
            ready((s, i))
        }

        type BarFut = Ready<String>;
        fn bar(self, _: context::Context, s: String) -> Self::BarFut {
            ready(s)
        }

        type BazFut = Ready<()>;
        fn baz(self, _: context::Context) -> Self::BazFut {
            ready(())
        }
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
        async fn one_arg(foo: String) -> i32;
        async fn two_args_no_return(bar: String, baz: u64);
        async fn two_args(bar: String, baz: u64) -> String;
        async fn no_args_ret_error() -> i32;
        async fn one_arg_ret_error(foo: String) -> String;
        async fn no_arg_implicit_return_error();
        #[doc = "attr"]
        async fn one_arg_implicit_return_error(foo: String);
    }
}
