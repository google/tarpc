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

#[allow(non_camel_case_types)]
#[test]
fn raw_idents() {
    use futures::future::{ready, Ready};

    type r#yield = String;

    #[tarpc::service]
    trait r#trait {
        async fn r#await(r#struct: r#yield, r#enum: i32) -> (r#yield, i32);
        async fn r#fn(r#impl: r#yield) -> r#yield;
        async fn r#async();
    }

    impl r#trait for () {
        type AwaitFut = Ready<(r#yield, i32)>;
        fn r#await(self, _: context::Context, r#struct: r#yield, r#enum: i32) -> Self::AwaitFut {
            ready((r#struct, r#enum))
        }

        type FnFut = Ready<r#yield>;
        fn r#fn(self, _: context::Context, r#impl: r#yield) -> Self::FnFut {
            ready(r#impl)
        }

        type AsyncFut = Ready<()>;
        fn r#async(self, _: context::Context) -> Self::AsyncFut {
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
