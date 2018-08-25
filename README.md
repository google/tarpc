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

[Documentation](https://docs.rs/crate/tarpc/)

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
tarpc = "0.12.0"
tarpc-plugins = "0.4.0"
```


The `service!` macro expands to a collection of items that form an
rpc service. In the above example, the macro is called within the
`hello_service` module. This module will contain `FutureClient` and `FutureService` traits.  There is
also a `FutureServiceExt` trait that provides starter `fn`s for services, with an
umbrella impl for all services.  These generated types make it easy and
ergonomic to write servers without dealing with sockets or serialization
directly. Simply implement one of the generated traits, and you're off to the
races! See the `tarpc_examples` package for more examples.

## Example:

Here's a small service.

```rust
#![feature(plugin)]
#![plugin(tarpc_plugins)]

extern crate futures;
#[macro_use]
extern crate tarpc;
extern crate tokio_core;

use futures::Future;
use tarpc::future::{client, server};
use tarpc::future::client::ClientExt;
use tarpc::util::FirstSocketAddr;
use tokio_core::reactor;

service! {
    rpc hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl FutureService for HelloServer {
    type HelloFut = Result<String, ()>;

    fn hello(&self, name: String) -> Self::HelloFut {
        Ok(format!("Hello, {}!", name))
    }
}

fn main() {
    let mut reactor = reactor::Core::new().unwrap();
    let (handle, server) = HelloServer.listen("localhost:10000".first_socket_addr(),
                                  &reactor.handle(),
                                  server::Options::default())
                          .unwrap();
    reactor.handle().spawn(server);
    let options = client::Options::default().handle(reactor.handle());
    reactor.run(FutureClient::connect(handle.addr(), options)
            .map_err(tarpc::Error::from)
            .and_then(|client| client.hello("Mom".to_string()))
            .map(|resp| println!("{}", resp)))
        .unwrap();
}
```

## Example: Futures + TLS

By default, tarpc internally uses a [`TcpStream`] for communication between your clients and
servers. However, TCP by itself has no encryption. As a result, your communication will be sent in
the clear. If you want your RPC communications to be encrypted, you can choose to use [TLS]. TLS
operates as an encryption layer on top of TCP. When using TLS, your communication will occur over a
[`TlsStream<TcpStream>`]. You can add the ability to make TLS clients and servers by adding `tarpc`
with the `tls` feature flag enabled.

When using TLS, some additional information is required. You will need to make [`TlsAcceptor`] and
`client::tls::Context` structs; `client::tls::Context` requires a [`TlsConnector`]. The
[`TlsAcceptor`] and [`TlsConnector`] types are defined in the [native-tls]. tarpc re-exports
external TLS-related types in its `native_tls` module (`tarpc::native_tls`).

[TLS]: https://en.wikipedia.org/wiki/Transport_Layer_Security
[`TcpStream`]: https://docs.rs/tokio-core/0.1/tokio_core/net/struct.TcpStream.html
[`TlsStream<TcpStream>`]: https://docs.rs/native-tls/0.1/native_tls/struct.TlsStream.html
[`TlsAcceptor`]: https://docs.rs/native-tls/0.1/native_tls/struct.TlsAcceptor.html
[`TlsConnector`]: https://docs.rs/native-tls/0.1/native_tls/struct.TlsConnector.html
[native-tls]: https://github.com/sfackler/rust-native-tls

Both TLS streams and TCP streams are supported in the same binary when the `tls` feature is enabled.
However, if you are working with both stream types, ensure that you use the TLS clients with TLS
servers and TCP clients with TCP servers.

```rust,no_run
#![feature(plugin)]
#![plugin(tarpc_plugins)]

extern crate futures;
#[macro_use]
extern crate tarpc;
extern crate tokio_core;

use futures::Future;
use tarpc::future::{client, server};
use tarpc::future::client::ClientExt;
use tarpc::tls;
use tarpc::util::FirstSocketAddr;
use tokio_core::reactor;
use tarpc::native_tls::{Pkcs12, TlsAcceptor};

service! {
    rpc hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl FutureService for HelloServer {
    type HelloFut = Result<String, ()>;

    fn hello(&self, name: String) -> Self::HelloFut {
        Ok(format!("Hello, {}!", name))
    }
}

fn get_acceptor() -> TlsAcceptor {
    let buf = include_bytes!("test/identity.p12");
    let pkcs12 = Pkcs12::from_der(buf, "password").unwrap();
    TlsAcceptor::builder(pkcs12).unwrap().build().unwrap()
}

fn main() {
    let mut reactor = reactor::Core::new().unwrap();
    let acceptor = get_acceptor();
    let (handle, server) = HelloServer.listen("localhost:10000".first_socket_addr(),
                                            &reactor.handle(),
                                            server::Options::default().tls(acceptor)).unwrap();
    reactor.handle().spawn(server);
    let options = client::Options::default()
                                   .handle(reactor.handle())
                                   .tls(tls::client::Context::new("foobar.com").unwrap());
    reactor.run(FutureClient::connect(handle.addr(), options)
            .map_err(tarpc::Error::from)
            .and_then(|client| client.hello("Mom".to_string()))
            .map(|resp| println!("{}", resp)))
        .unwrap();
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
