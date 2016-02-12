## tarpc: Tim & Adam's RPC lib
[![Travis-CI
Status](https://travis-ci.org/google/tarpc.png?branch=master)](https://travis-ci.org/google/tarpc)

*Disclaimer*: This is not an official Google product.

tarpc is an RPC framework for rust with a focus on ease of use. Defining and implementing an echo-like server can be done in just a few lines of code:

```rust
#![feature(custom_derive, plugin)]
#![plugin(serde_macros)]
#[macro_use]
extern crate tarpc;
extern crate serde;

mod hello_service {
    service! {
        rpc hello(name: String) -> String;
    }
}

struct HelloService;
impl hello_service::Service for HelloService {
    fn hello(&self, name: String) -> String {
        format!("Hello, {}!", s)
    }
}

fn main() {
    let server_handle = hello_service::serve("0.0.0.0:0", HelloService, None).unwrap();
    let client = hello_service::Client::new(server_handle.local_addr(), None).unwrap();
    assert_eq!("Hello, Mom!".into(), client.hello("Mom".into()).unwrap());
    drop(client);
    server_handle.shutdown();
}
```

The `service!` macro expands to a collection of items that collectively form an rpc service. In the
above example, the macro is called within the `hello_service` module. This module will contain a
`Client` type, a `Service` trait, and a `serve` function. `serve` can be used to start a server
listening on a tcp port. A `Client` can connect to such a service. Any type implementing the
`Service` trait can be passed to `serve`. These generated types are specific to the echo service,
and make it easy and ergonomic to write servers without dealing with sockets or serialization
directly. See the tarpc_examples package for more sophisticated examples.

## Additional Features
- Attributes can be specified on rpc methods. These will be included on both the `Service` trait
  methods as well as on the `Client`'s stub methods.
- Just like regular fns, the return type can be left off when it's `-> ()`.
- Arg-less rpc's are also allowed.

## Planned Improvements (actively being worked on)
- Automatically reconnect on the client side when the connection cuts out.
- Support asynchronous server implementations (currently thread per connection).
- Support generic serialization protocols.

## Contributing

To contribute to tarpc, please see [CONTRIBUTING](CONTRIBUTING.md).

## License

tarpc is distributed under the terms of the MIT license.

See [LICENSE](LICENSE) for details.
