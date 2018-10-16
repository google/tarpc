#![feature(
    futures_api,
    pin,
    arbitrary_self_types,
    await_macro,
    async_await,
    proc_macro_hygiene,
)]

// This is the service definition. It looks a lot like a trait definition.
// It defines one RPC, hello, which takes one arg, name, and returns a String.
tarpc::service! {
    /// Returns a greeting for name.
    rpc hello(name: String) -> String;
}
