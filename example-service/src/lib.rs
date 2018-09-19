#![feature(plugin, futures_api, pin, arbitrary_self_types, await_macro, async_await, existential_type)]
#![plugin(tarpc_plugins)]

tarpc::service! {
    /// Returns a greeting for name.
    rpc hello(name: String) -> String;
}
