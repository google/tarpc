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
tarpc = "0.7.2"
tarpc-plugins = "0.1.1"
```

## Example: Sync

tarpc has two APIs: `sync` for blocking code and `future` for asynchronous
code. First we'll discuss how to use the sync API. The following example shows
how to build a client and server binary and run them on the command line. Code
for these examples involves three crates: `examples/hello_api`,
`examples/sync_server`, and `examples/sync_cli`.

The first file is `lib.rs` in the `hello_api` crate, which defines the RPC
service.  The `service!` macro expands to a collection
of items that form an rpc service. This module will contain `SyncClient`, and
`FutureClient` types, and a `SyncService`. There is also a `ServiceExt` trait
that provides starter `fn`s for services, with an umbrella impl for all
services.  These generated types make it easy and ergonomic to write servers
without dealing with sockets or serialization directly. Simply implement one of
the generated traits, and you're off to the races!

```rust
#![feature(plugin)]
#![plugin(tarpc_plugins)]

#[macro_use]
extern crate tarpc;

service! {
    rpc hello(name: String) -> String;
}
```

Then, in `main.rs` in the `sync_server` crate, we create an implementation for
the `hello` RPC, and a simple main function that starts the server given a
port.

```rust
extern crate hello_api;
extern crate tarpc;
extern crate clap;

use hello_api::SyncServiceExt;
use tarpc::sync::server::Options;
use tarpc::util::Never;

#[derive(Clone)]
pub struct HelloServer;

impl hello_api::SyncService for HelloServer {
    fn hello(&self, name: String) -> Result<String, Never> {
        Ok(format!("Hey {}!", name))
    }
}

fn main() {
    let matches = clap::App::new("hello sync server")
        .arg(clap::Arg::with_name("port").required(true))
        .get_matches();
    let port: u16 = matches.value_of("port").unwrap().parse().unwrap();
    let handle = HelloServer.listen(format!("localhost:{}", port), Options::default()).unwrap();
    println!("Listening on {}", handle.addr());
    handle.run();
}
```

So now we've implemented the service, and packaged that implementation into a
separate runnable binary.

Next is the command line interface, which lives in the `examples/sync_cli`
crate. This client allows you to specify a server address and a person's name,
and sends a `hello` RPC with that person's name to the specified server. For
simplicity this binary uses `clap` to handle command line arguments, though
it's not required.

```rust
extern crate clap;
extern crate tarpc;
extern crate hello_api;

use tarpc::sync::client::{Options, ClientExt};

fn main() {
    let matches = clap::App::new("hello sync cli")
        .arg(clap::Arg::with_name("server_address").required(true))
        .arg(clap::Arg::with_name("person_name").required(true))
        .get_matches();
    let addr = matches.value_of("server_address").unwrap();
    let person_name = matches.value_of("person_name").unwrap();
    let client = hello_api::SyncClient::connect(addr, Options::default()).unwrap();
    println!("{}", client.hello(person_name.into()).unwrap());
}
```

Code for this example can be found in `examples/sync_starter/src`. To run it do
the following:

```
$ cd examples/sync_server
$ cargo run 10000
Listening on 127.0.0.1:10000
```

Now the server is listening on port `10000`. Note that it's possible that you
already have something listening on that port on your machine. In that case,
just choose another port. We can then use the generated client to send an RPC
to the server:

```
$ cd examples/sync_cli
$ cargo run localhost:10000 Mom
Hey Mom!
```

## Example: Futures

Here's the same service, implemented using futures. You might want to use
futures if performance is the top priority. The futures API adds almost no
overhead, and provides the maximum control over how code is executed.

You'll notice that we don't have to repeat the service definition, because
it's already in `hello_api` which generates traits and helper types generated
for both sync and future APIs just from the one macro invocation. The main
difference is that we now return a future from service implementation methods.
This implementation lives in the `examples/future_server` crate.

```rust
extern crate clap;
extern crate hello_api;
extern crate tarpc;
extern crate tokio_core;

use hello_api::{FutureServiceExt, FutureService};
use tarpc::future::server::Options;
use tarpc::util::{Never, FirstSocketAddr};

#[derive(Clone)]
pub struct HelloServer;

impl FutureService for HelloServer {
    type HelloFut = Result<String, Never>;

    fn hello(&self, name: String) -> Self::HelloFut {
        Ok(format!("Hello, {}!", name))
    }
}

fn main() {
    let matches = clap::App::new("hello future server")
        .arg(clap::Arg::with_name("port").required(true))
        .get_matches();
    let port: u16 = matches.value_of("port").unwrap().parse().unwrap();
    let mut reactor = tokio_core::reactor::Core::new().unwrap();
    let (handle, server) = HelloServer.listen(format!("localhost:{}", port).first_socket_addr(),
                &reactor.handle(),
                Options::default())
        .unwrap();
    println!("Listening on {}", handle.addr());
    reactor.run(server).unwrap();
}
```

In this example, `hello` returns `Result`, but in more complex servers, you
might talk to another server or do something using IO. In those cases, the
`HelloFut` type would be something more complex. An example of such a server
can be found in `examples/two_servers.rs`. In cases where RPC handlers are very
fast and don't block, implementing `FutureService` is a good choice, since
there's no threading overhead. However, blocking in an RPC method
implementation would cause the reactor core running the server to grind to a
halt, which would prevent new RPCs from being served. Blocking operations can
still be used, but their execution must be delegated to a thread. A common way
to do that is to use `futures-cpupool`.

The `listen` method returns the server handle and the server future. The handle
contains the bound address, and other functionality for manipulating a running
server.Running this future on a reactor core causes the server to serve
incoming requests.  Having the future itself returned gives you the flexibility
to run the server either using a reactor handle (`handle.spawn()`), or by
running it on the reactor directly. Spawning the server using handle is useful
if you might run multiple servers on the same reactor. Running it using
`reactor.run()` directly is useful for blocking the thread until the server is
done. After calling `listen` we need to run the server on a reactor core. The
returned future will not resolve unless the server is shut down explicitly. In
this particular code snippet, it will run forever. Next is the command line
interface in the `examples/future_cli` crate.

```rust
extern crate clap;
extern crate futures;
extern crate hello_api;
extern crate tarpc;
extern crate tokio_core;

use futures::Future;
use tarpc::future::client::{Options, ClientExt};
use tarpc::util::FirstSocketAddr;

fn main() {
    let matches = clap::App::new("hello future cli")
        .arg(clap::Arg::with_name("server_address").required(true))
        .arg(clap::Arg::with_name("person_name").required(true))
        .get_matches();
    let addr = matches.value_of("server_address").unwrap().first_socket_addr();
    let person_name = matches.value_of("person_name").unwrap();
    let mut reactor = tokio_core::reactor::Core::new().unwrap();
    reactor.run(hello_api::FutureClient::connect(addr, Options::default())
            .map_err(tarpc::Error::from)
            .and_then(|client| client.hello(person_name.into()))
            .map(|resp| println!("{}", resp)))
        .unwrap();
}
```

Again, this is similar to the sync version. The main difference again is that
we use future combinators to build up a future to send the RPC and then execute
it on a reactor core. Running this code is the same as with the sync code:


```
$ cd examples/future_server
$ cargo run 10000
Listening on 127.0.0.1:10000
```

Now the server is listening on port `10000`. Note that it's possible that you
already have something listening on that port on your machine. In that case,
just choose another port. We can then use the generated client to send an RPC
to the server:

```
$ cd examples/future_cli
$ cargo run localhost:10000 Mom
Hey Mom!
```

Since the sync and future APIs both implement the same protocol underneath,
they are both compatible with each other. The sync server can handle queries
from future clients, and the future server can handle queries from sync
clients.

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
use tarpc::{client, server};
use tarpc::client::future::ClientExt;
use tarpc::util::{FirstSocketAddr, Never};
use tokio_core::reactor;
use tarpc::native_tls::{Pkcs12, TlsAcceptor};

service! {
    rpc hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl FutureService for HelloServer {
    type HelloFut = Result<String, Never>;

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
    let (addr, server) = HelloServer.listen("localhost:10000".first_socket_addr(),
                                            &reactor.handle(),
                                            server::Options::default().tls(acceptor)).unwrap();
    reactor.handle().spawn(server);
    let options = client::Options::default()
                                   .handle(reactor.handle())
                                   .tls(client::tls::Context::new("foobar.com").unwrap());
    reactor.run(FutureClient::connect(addr, options)
            .map_err(tarpc::Error::from)
            .and_then(|client| client.hello("Mom".to_string()))
            .map(|resp| println!("{}", resp)))
        .unwrap();
}
```

## Tips

### Sync vs Futures

A single `service!` invocation generates code for both synchronous and future-based applications.
It's up to the user whether they want to implement the sync API or the futures API. The sync API has
the simplest programming model, at the cost of some overhead - each RPC is handled in its own
thread. The futures API is based on tokio and can run on any tokio-compatible executor. This mean a
service that implements the futures API for a tarpc service can run on a single thread, avoiding
context switches and the memory overhead of having a thread per RPC.

### Errors

All generated tarpc RPC methods return either `tarpc::Result<T, E>` or something like `Future<T,
E>`. The error type defaults to `tarpc::util::Never` (a wrapper for `!` which implements
`std::error::Error`) if no error type is explicitly specified in the `service!` macro invocation. An
error type can be specified like so:

```rust,ignore
use tarpc::util::Message;

service! {
    rpc hello(name: String) -> String | Message
}
```

`tarpc::util::Message` is just a wrapper around string that implements `std::error::Error` provided
for service implementations that don't require complex error handling. The pipe is used as syntax
for specifying the error type in a way that's agnostic of whether the service implementation is
synchronous or future-based. Note that in the simpler examples in the readme, no pipe is used, and
the macro automatically chooses `tarpc::util::Never` as the error type.

The above declaration would produce the following synchronous service trait:

```rust,ignore
trait SyncService {
    fn hello(&self, name: String) -> Result<String, Message>;
}
```

and the following future-based trait:

```rust,ignore
trait FutureService {
    type HelloFut: IntoFuture<String, Message>;

    fn hello(&mut self, name: String) -> Self::HelloFut;
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
