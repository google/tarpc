## tarpc: Tim & Adam's RPC lib
[![Travis-CI Status](https://travis-ci.org/google/tarpc.png?branch=master)](https://travis-ci.org/google/tarpc)
[![Coverage Status](https://coveralls.io/repos/github/google/tarpc/badge.svg?branch=master)](https://coveralls.io/github/google/tarpc?branch=master)
[![Software License](https://img.shields.io/badge/license-MIT-brightgreen.svg)](LICENSE.txt)
[![Latest Version](https://img.shields.io/crates/v/tarpc.svg)](https://crates.io/crates/tarpc)
[![Join the chat at https://gitter.im/tarpc/Lobby](https://badges.gitter.im/tarpc/Lobby.svg)](https://gitter.im/tarpc/Lobby?utm_source=badge&utm_medium=badge&utm_campaign=pr-badge&utm_content=badge)

*Disclaimer*: This is not an official Google product.

tarpc is an RPC framework for rust with a focus on ease of use. Defining a service can be done in
just a few lines of code, and most of the boilerplate of writing a server is taken care of for you.

[Documentation](https://docs.rs/tarpc/0.6.0/tarpc/)

## What is an RPC framework?
"RPC" stands for "Remote Procedure Call," a function call where the work of producing the return
value is being done somewhere else. When an rpc function is invoked, behind the scenes the function
contacts some other process somewhere and asks them to compute the function instead. The original
function then returns the value produced by that other server.

[More information](https://www.cs.cf.ac.uk/Dave/C/node33.html)

## Usage
Add to your `Cargo.toml` dependencies:

```toml
tarpc = "0.6"
```

## Example
```rust
#[macro_use]
extern crate tarpc;

mod hello_service {
    service! {
        rpc hello(name: String) -> String;
    }
}
use hello_service::Service as HelloService;

struct HelloServer;
impl HelloService for HelloServer {
    fn hello(&self, name: String) -> String {
        format!("Hello, {}!", name)
    }
}

fn main() {
    let addr = "localhost:10000";
    let server_handle = HelloServer.spawn(addr).unwrap();
    let client = hello_service::Client::new(addr).unwrap();
    assert_eq!("Hello, Mom!", client.hello("Mom".into()).unwrap());
    drop(client);
    server_handle.shutdown();
}
```

The `service!` macro expands to a collection of items that collectively form an rpc service. In the
above example, the macro is called within the `hello_service` module. This module will contain a
`Client` (and `AsyncClient`) type, and a `Service` trait. The trait provides default `fn`s for
starting the service: `spawn` and `spawn_with_config`, which start the service listening over an
arbitrary transport. A `Client` (or `AsyncClient`) can connect to such a service. These generated
types make it easy and ergonomic to write servers without dealing with sockets or serialization
directly. See the tarpc_examples package for more sophisticated examples.

## Documentation
Use `cargo doc` as you normally would to see the documentation created for all
items expanded by a `service!` invocation.

## Additional Features
- Connect over any transport that `impl`s the `Transport` trait.
- Concurrent requests from a single client.
- Any type that `impl`s `serde`'s `Serialize` and `Deserialize` can be used in the rpc signatures.
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
