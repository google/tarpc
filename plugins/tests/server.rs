use assert_type_eq::assert_type_eq;
use futures::Future;
use std::pin::Pin;
use tarpc::context;

// these need to be out here rather than inside the function so that the
// assert_type_eq macro can pick them up.
#[tarpc::service]
trait Foo {
    async fn two_part(s: String, i: i32) -> (String, i32);
    async fn bar(s: String) -> String;
    async fn baz();
}

#[test]
fn type_generation_works() {
    #[tarpc::server]
    impl Foo for () {
        async fn two_part(self, _: context::Context, s: String, i: i32) -> (String, i32) {
            (s, i)
        }

        async fn bar(self, _: context::Context, s: String) -> String {
            s
        }

        async fn baz(self, _: context::Context) {}
    }

    // the assert_type_eq macro can only be used once per block.
    {
        assert_type_eq!(
            <() as Foo>::TwoPartFut,
            Pin<Box<dyn Future<Output = (String, i32)> + Send>>
        );
    }
    {
        assert_type_eq!(
            <() as Foo>::BarFut,
            Pin<Box<dyn Future<Output = String> + Send>>
        );
    }
    {
        assert_type_eq!(
            <() as Foo>::BazFut,
            Pin<Box<dyn Future<Output = ()> + Send>>
        );
    }
}

#[allow(non_camel_case_types)]
#[test]
fn raw_idents_work() {
    type r#yield = String;

    #[tarpc::service]
    trait r#trait {
        async fn r#await(r#struct: r#yield, r#enum: i32) -> (r#yield, i32);
        async fn r#fn(r#impl: r#yield) -> r#yield;
        async fn r#async();
    }

    #[tarpc::server]
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

    #[tarpc::server]
    impl Syntax for () {
        #[deny(warnings)]
        #[allow(non_snake_case)]
        async fn TestCamelCaseDoesntConflict(self, _: context::Context) {}

        async fn hello(self, _: context::Context) -> String {
            String::new()
        }

        async fn attr(self, _: context::Context, _s: String) -> String {
            String::new()
        }

        async fn no_args_no_return(self, _: context::Context) {}

        async fn no_args(self, _: context::Context) -> () {}

        async fn one_arg(self, _: context::Context, _one: String) -> i32 {
            0
        }

        async fn two_args_no_return(self, _: context::Context, _one: String, _two: u64) {}

        async fn two_args(self, _: context::Context, _one: String, _two: u64) -> String {
            String::new()
        }

        async fn no_args_ret_error(self, _: context::Context) -> i32 {
            0
        }

        async fn one_arg_ret_error(self, _: context::Context, _one: String) -> String {
            String::new()
        }

        async fn no_arg_implicit_return_error(self, _: context::Context) {}

        async fn one_arg_implicit_return_error(self, _: context::Context, _one: String) {}
    }
}
