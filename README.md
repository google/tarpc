## tarpc: Tim & Adam's RPC lib
[![Travis-CI Status](https://travis-ci.org/google/tarpc.svg?branch=master)](https://travis-ci.org/google/tarpc)
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
Add to your `Cargo.toml` dependencies:

```toml
tarpc = "0.18.0"
```

The `tarpc::service` attribute expands to a collection of items that form an rpc service.
These generated types make it easy and ergonomic to write servers with less boilerplate.
Simply implement the generated service trait, and you're off to the races!

## Example

For this example, in addition to tarpc, also add two other dependencies to
your `Cargo.toml`:

```toml
futures-preview = { version = "0.3.0-alpha.17", features = ["compat"] }
tokio = "0.1"
```

In the following example, we use an in-process channel for communication between
client and server. In real code, you will likely communicate over the network.
For a more real-world example, see [example-service](example-service).

First, let's set up the dependencies and service definition.

```rust
#![feature(async_await, proc_macro_hygiene)]
# extern crate futures;

use futures::{
    compat::Executor01CompatExt,
    future::{self, Ready},
    prelude::*,
};
use tarpc::{
    client, context,
    server::{self, Handler},
};
use std::io;

// This is the service definition. It looks a lot like a trait definition.
// It defines one RPC, hello, which takes one arg, name, and returns a String.
#[tarpc::service]
trait World {
    /// Returns a greeting for name.
    async fn hello(name: String) -> String;
}
```

This service definition generates a trait called `World`. Next we need to
implement it for our Server struct.

```rust
# #![feature(async_await, proc_macro_hygiene)]
# extern crate futures;
#
# use futures::{
#     compat::Executor01CompatExt,
#     future::{self, Ready},
#     prelude::*,
# };
# use tarpc::{
#     client, context,
#     server::{self, Handler},
# };
# use std::io;
#
# // This is the service definition. It looks a lot like a trait definition.
# // It defines one RPC, hello, which takes one arg, name, and returns a String.
# #[tarpc::service]
# trait World {
#     /// Returns a greeting for name.
#     async fn hello(name: String) -> String;
# }
#
// This is the type that implements the generated World trait. It is the business logic
// and is used to start the server.
#[derive(Clone)]
struct HelloServer;

impl World for HelloServer {
    // Each defined rpc generates two items in the trait, a fn that serves the RPC, and
    // an associated type representing the future output by the fn.

    type HelloFut = Ready<String>;

    fn hello(self, _: context::Context, name: String) -> Self::HelloFut {
        future::ready(format!("Hello, {}!", name))
    }
}
```

Lastly let's write our `main` that will start the server. While this example uses an
[in-process
channel](https://docs.rs/tarpc/0.18.0/tarpc/transport/channel/struct.UnboundedChannel.html),
tarpc also ships a
[transport](https://docs.rs/tarpc-bincode-transport/0.7.0/tarpc_bincode_transport/)
that uses bincode over TCP.

```rust
# #![feature(async_await, proc_macro_hygiene)]
# extern crate futures;
#
# use futures::{
#     future::{self, Ready},
#     prelude::*,
# };
# use tarpc::{
#     client, context,
#     server::{self, Handler},
# };
# use std::io;
#
# // This is the service definition. It looks a lot like a trait definition.
# // It defines one RPC, hello, which takes one arg, name, and returns a String.
# #[tarpc::service]
# trait World {
#     /// Returns a greeting for name.
#     async fn hello(name: String) -> String;
# }
#
# // This is the type that implements the generated World trait. It is the business logic
# // and is used to start the server.
# #[derive(Clone)]
# struct HelloServer;
#
# impl World for HelloServer {
#     // Each defined rpc generates two items in the trait, a fn that serves the RPC, and
#     // an associated type representing the future output by the fn.
#
#     type HelloFut = Ready<String>;
#
#     fn hello(self, _: context::Context, name: String) -> Self::HelloFut {
#         future::ready(format!("Hello, {}!", name))
#     }
# }
#
#[runtime::main(runtime_tokio::Tokio)]
async fn main() -> io::Result<()> {
    let (client_transport, server_transport) = tarpc::transport::channel::unbounded();

    let server = server::new(server::Config::default())
        // incoming() takes a stream of transports such as would be returned by
        // TcpListener::incoming (but a stream instead of an iterator).
        .incoming(stream::once(future::ready(server_transport)))
        .respond_with(HelloServer.serve());

    let _ = runtime::spawn(server);

    // WorldClient is generated by the macro. It has a constructor `new` that takes a config and
    // any Transport as input
    let mut client = WorldClient::new(client::Config::default(), client_transport).await?;

    // The client has an RPC method for each RPC defined in the annotated trait. It takes the same
    // args as defined, with the addition of a Context, which is always the first arg. The Context
    // specifies a deadline and trace information which can be helpful in debugging requests.
    let hello = client.hello(context::current(), "Stim".to_string()).await?;

    println!("{}", hello);

    Ok(())
}
```

## Service Documentation

Use `cargo doc` as you normally would to see the documentation created for all
items expanded by a `service!` invocation.
