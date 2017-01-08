## tarpc: Tim & Adam's RPC lib
[![Travis-CI Status](https://travis-ci.org/google/tarpc.png?branch=master)](https://travis-ci.org/google/tarpc)
[![Coverage Status](https://coveralls.io/repos/github/google/tarpc/badge.svg?branch=master)](https://coveralls.io/github/google/tarpc?branch=master)
[![Software License](https://img.shields.io/badge/license-MIT-brightgreen.svg)](LICENSE)
[![Latest Version](https://img.shields.io/crates/v/tarpc.svg)](https://crates.io/crates/tarpc)
[![Join the chat at https://gitter.im/tarpc/Lobby](https://badges.gitter.im/tarpc/Lobby.svg)](https://gitter.im/tarpc/Lobby?utm_source=badge&utm_medium=badge&utm_campaign=pr-badge&utm_content=badge)

*Disclaimer*: This is not an official Google product.

tarpc is an RPC framework for rust with a focus on ease of use. Defining a
service can be done in just a few lines of code, and most of the boilerplate of
writing a server is taken care of for you.

[Documentation](https://docs.rs/tarpc)

## What is an RPC framework?
"RPC" stands for "Remote Procedure Call," a function call where the work of
producing the return value is being done somewhere else. When an rpc function is
invoked, behind the scenes the function contacts some other process somewhere
and asks them to evaluate the function instead. The original function then
returns the value produced by the other process.

RPC frameworks are a fundamental building block of most microservices-oriented
architectures. Two well-known ones are [gRPC](http://www.grpc.io) and
[Cap'n Proto](https://capnproto.org/).

tarpc differentiates itself from other RPC frameworks by defining the schema in code,
rather than in a separate language such as .proto. This means there's no separate compilation
process, and no cognitive context switching between different languages. Additionally, it
works with the community-backed library serde: any serde-serializable type can be used as
arguments to tarpc fns.

## Usage
**NB**: *this example is for master. Are you looking for other
[versions](https://docs.rs/tarpc)?*

Add to your `Cargo.toml` dependencies:

```toml
tarpc = { git = "https://github.com/google/tarpc" }
tarpc-plugins = { git = "https://github.com/google/tarpc" }
```

## Example
```rust
// required by `FutureClient` (not used in this example)
#![feature(conservative_impl_trait, plugin)]
#![plugin(tarpc_plugins)]

extern crate futures;
#[macro_use]
extern crate tarpc;

use tarpc::util::Never;
use tarpc::sync::Connect;

service! {
    rpc hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl SyncService for HelloServer {
    fn hello(&mut self, name: String) -> Result<String, Never> {
        Ok(format!("Hello, {}!", name))
    }
}

fn main() {
    let addr = "localhost:10000";
    HelloServer.listen(addr).unwrap();
    let client = SyncClient::connect(addr).unwrap();
    println!("{}", client.hello("Mom".to_string()).unwrap());
}
```

The `service!` macro expands to a collection of items that form an
rpc service. In the above example, the macro is called within the
`hello_service` module. This module will contain `SyncClient`, `AsyncClient`,
and `FutureClient` types, and `SyncService` and `AsyncService` traits.  There is
also a `ServiceExt` trait that provides starter `fn`s for services, with an
umbrella impl for all services.  These generated types make it easy and
ergonomic to write servers without dealing with sockets or serialization
directly. Simply implement one of the generated traits, and you're off to the
races! See the tarpc_examples package for more examples.

## Example: Futures

Here's the same service, implemented using futures.

```rust
#![feature(conservative_impl_trait, plugin)]
#![plugin(tarpc_plugins)]

extern crate futures;
#[macro_use]
extern crate tarpc;
extern crate tokio_core;

use futures::Future;
use tarpc::future::Connect;
use tarpc::util::{FirstSocketAddr, Never};
use tokio_core::reactor;

service! {
    rpc hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl FutureService for HelloServer {
    type HelloFut = futures::Finished<String, Never>;

    fn hello(&mut self, name: String) -> Self::HelloFut {
        futures::finished(format!("Hello, {}!", name))
    }
}

fn main() {
    let addr = "localhost:10000".first_socket_addr();
    let mut core = reactor::Core::new().unwrap();
    HelloServer.listen_with(addr, core.handle()).unwrap();
    core.run(
        FutureClient::connect(&addr)
            .map_err(tarpc::Error::from)
            .and_then(|mut client| client.hello("Mom".to_string()))
            .map(|resp| println!("{}", resp))
    ).unwrap();
}
```

## Example: TLS + Sync

Instead of using a `TcpStream`, a `TlsStream<TcpStream>` will be used instead
when the `tls` feature of `tarpc` is enabled. When using TLS, some additional
information is required from the user. You will need to be make the
`TlsAcceptor` and `TlsClientContext` structs. The `TlsAcceptor` and
`TlsConnector` (which is required in `TlsClientContext`) structs are defined in
the [native-tls](https://github.com/sfackler/rust-native-tls) crate which is
exposed by `tarpc`.

```rust
#![feature(conservative_impl_trait, plugin)]
#![plugin(tarpc_plugins)]

#[macro_use]
extern crate tarpc;

use tarpc::sync::Connect;
use tarpc::util::Never;
use tarpc::TlsClientContext;
use tarpc::native_tls::{Pkcs12, TlsAcceptor, TlsConnector};

service! {
    rpc hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl SyncService for HelloServer {
    fn hello(&mut self, name: String) -> Result<String, Never> {
        Ok(format!("Hello, {}!", name))
    }
}

fn tls_context() -> (TlsAcceptor, TlsClientContext) {
     let buf = include_bytes!("test/identity.p12");
     let pkcs12 = Pkcs12::from_der(buf, "password").unwrap();
     let acceptor = TlsAcceptor::builder(pkcs12).unwrap().build().unwrap();
     let client_cx = TlsClientContext::try_new("foobar.com").unwrap();

     (acceptor, client_cx)
}

fn main() {
    let addr = "localhost:10000";
    let (acceptor, client_cx) = tls_context();
    let _server = HelloServer.listen(addr, acceptor);
    let mut client = SyncClient::connect(addr, client_cx).unwrap();
    println!("{}", client.hello("Mom".to_string()).unwrap());
}
```

## Example: Futures + TLS

Here's the same TLS service, implemented using futures.

```rust
#![feature(conservative_impl_trait, plugin)]
#![plugin(tarpc_plugins)]

extern crate futures;
#[macro_use]
extern crate tarpc;
extern crate tokio_core;

use futures::Future;
use tarpc::future::Connect;
use tarpc::util::{FirstSocketAddr, Never};
use tokio_core::reactor;

service! {
    rpc hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl FutureService for HelloServer {
    type HelloFut = futures::Finished<String, Never>;

    fn hello(&mut self, name: String) -> Self::HelloFut {
        futures::finished(format!("Hello, {}!", name))
    }
}

fn tls_context() -> (TlsAcceptor, TlsClientContext) {
     let buf = include_bytes!("test/identity.p12");
     let pkcs12 = Pkcs12::from_der(buf, "password").unwrap();
     let acceptor = TlsAcceptor::builder(pkcs12).unwrap().build().unwrap();
     let client_cx = TlsClientContext::try_new("foobar.com").unwrap();

     (acceptor, client_cx)
}

fn main() {
    let addr = "localhost:10000".first_socket_addr();
    let mut core = reactor::Core::new().unwrap();
    let (acceptor, client_cx) = tls_context();
    HelloServer.listen_with(addr, core.handle(), acceptor).unwrap();
    core.run(
        FutureClient::connect(&addr, client_cx)
            .map_err(tarpc::Error::from)
            .and_then(|mut client| client.hello("Mom".to_string()))
            .map(|resp| println!("{}", resp))
    ).unwrap();
}
```

## Documentation
Use `cargo doc` as you normally would to see the documentation created for all
items expanded by a `service!` invocation.

## Additional Features
- Concurrent requests from a single client.
- Compatible with tokio services.
- Run any number of clients and services on a single event loop.
- Any type that `impl`s `serde`'s `Serialize` and `Deserialize` can be used in
  rpc signatures.
- Attributes can be specified on rpc methods. These will be included on both the
  services' trait methods as well as on the clients' stub methods.

## Gaps/Potential Improvements (not necessarily actively being worked on)
- Configurable server rate limiting.
- Automatic client retries with exponential backoff when server is busy.
- Load balancing
- Service discovery
- Automatically reconnect on the client side when the connection cuts out.
- Support generic serialization protocols.

## Contributing

To contribute to tarpc, please see [CONTRIBUTING](CONTRIBUTING.md).

## License

tarpc is distributed under the terms of the MIT license.

See [LICENSE](LICENSE) for details.
