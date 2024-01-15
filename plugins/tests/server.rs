// these need to be out here rather than inside the function so that the
// assert_type_eq macro can pick them up.
#[tarpc::service]
trait Foo {
    async fn two_part(s: String, i: i32) -> (String, i32);
    async fn bar(s: String) -> String;
    async fn baz();
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
