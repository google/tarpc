## tarpc: Tim & Adam's RPC lib
[![Travis-CI Status](https://travis-ci.org/google/tarpc.png?branch=master)](https://travis-ci.org/google/tarpc)
[![Coverage Status](https://coveralls.io/repos/github/google/tarpc/badge.svg?branch=master)](https://coveralls.io/github/google/tarpc?branch=master)
[![Software License](https://img.shields.io/badge/license-MIT-brightgreen.svg)](LICENSE.txt)
[![Latest Version](https://img.shields.io/crates/v/tarpc.svg)](https://crates.io/crates/tarpc)

*Disclaimer*: This is not an official Google product.

tarpc is an RPC framework for rust with a focus on ease of use. Defining a
service can be done in just a few lines of code, and most of the boilerplate of
writing a server is taken care of for you.

[Documentation](https://google.github.io/tarpc)

## What is an RPC framework?
"RPC" stands for "Remote Procedure Call," a function call where the work of
producing the return value is being done somewhere else. When an rpc function is
invoked, behind the scenes the function contacts some other process somewhere
and asks them to compute the function instead. The original function then
returns the value produced by that other server.

RPC frameworks are a fundamental building block of most microservices-oriented
architectures. Two well-known ones are [gRPC](http://www.grpc.io) and
[Cap'n Proto](https://capnproto.org/).

## Usage
Add to your `Cargo.toml` dependencies:

```toml
tarpc = "0.5.0"
```

## Example
```rust
#![feature(default_type_parameter_fallback)]
#[macro_use]
extern crate tarpc;

use tarpc::Client;

service! {
    rpc hello(name: String) -> String;
}

#[derive(Clone, Copy)]
struct HelloServer;

impl SyncService for HelloServer {
    fn hello(&self, name: String) -> tarpc::RpcResult<String> {
        Ok(format!("Hello, {}!", name))
    }
}

fn main() {
    let addr = "localhost:10000";
    HelloServer.listen(addr).unwrap();
    let client = SyncClient::connect(addr).unwrap();
    assert_eq!("Hello, Mom!", client.hello(&"Mom".to_string()).unwrap());
}

```

The `service!` macro expands to a collection of items that collectively form an
rpc service. In the above example, the macro is called within the
`hello_service` module. This module will contain `SyncClient`, `AsyncClient`,
and `FutureClient` types, and `SyncService` and `AsyncService` traits.  There is
also a `ServiceExt` trait that provides starter `fn`s for services, with an
umbrella impl for all services.  These generated types make it easy and
ergonomic to write servers without dealing with sockets or serialization
directly. Simply implement one of the generated traits, and you're off to the
races! See the tarpc_examples package for more examples.

## Documentation
Use `cargo doc` as you normally would to see the documentation created for all
items expanded by a `service!` invocation.

## Additional Features
- Configurable server rate limiting.
- Automatic client retries with exponential backoff when server is busy.
- Concurrent requests from a single client.
- Backed by an mio `EventLoop`, protecting services (including `SyncService`s)
  from slowloris attacks.
- Run any number of clients on a single client event loop.
- Run any number of services on a single service event loop.
- Configure clients and services to run on a custom event loop, defaulting to
  the global event loop.
- Any type that `impl`s `serde`'s `Serialize` and `Deserialize` can be used in
  the rpc signatures.
- Attributes can be specified on rpc methods. These will be included on both the
  services' trait methods as well as on the clients' stub methods.
- Just like regular fns, the return type can be left off when it's `-> ()`.
- Arg-less rpc's are also allowed.

## Gaps/Potential Improvements (not necessarily actively being worked on)
- Multithreaded support.
- Load balancing
- Service discovery
- Automatically reconnect on the client side when the connection cuts out.
- Support generic serialization protocols.

## Contributing

To contribute to tarpc, please see [CONTRIBUTING](CONTRIBUTING.md).

## License

tarpc is distributed under the terms of the MIT license.

See [LICENSE](LICENSE) for details.
