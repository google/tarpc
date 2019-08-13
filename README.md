[![Build Status](https://travis-ci.org/google/tarpc.svg?branch=master)](https://travis-ci.org/google/tarpc)
[![Latest Version](https://img.shields.io/crates/v/tarpc.svg)](https://crates.io/crates/tarpc)
[![Join the chat at https://gitter.im/tarpc/Lobby](https://badges.gitter.im/tarpc/Lobby.svg)](https://gitter.im/tarpc/Lobby?utm_source=badge&utm_medium=badge&utm_campaign=pr-badge&utm_content=badge)

# tarpc

*Disclaimer*: This is not an official Google product.

tarpc is an RPC framework for rust with a focus on ease of use. Defining a
service can be done in just a few lines of code, and most of the boilerplate of
writing a server is taken care of for you.

[Documentation](https://docs.rs/crate/tarpc/)

### What is an RPC framework?
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
process, and no context switching between different languages.

Some other features of tarpc:
- Pluggable transport: any type impling `Stream<Item = Request> + Sink<Response>` can be
  used as a transport to connect the client and server.
- `Send` optional: if the transport doesn't require it, neither does tarpc!
- Cascading cancellation: dropping a request will send a cancellation message to the server.
  The server will cease any unfinished work on the request, subsequently cancelling any of its
  own requests, repeating for the entire chain of transitive dependencies.
- Configurable deadlines and deadline propagation: request deadlines default to 10s if
  unspecified. The server will automatically cease work when the deadline has passed. Any
  requests sent by the server that use the request context will propagate the request deadline.
  For example, if a server is handling a request with a 10s deadline, does 2s of work, then
  sends a request to another server, that server will see an 8s deadline.
- Serde serialization: enabling the `serde1` Cargo feature will make service requests and
  responses `Serialize + Deserialize`. It's entirely optional, though: in-memory transports can
  be used, as well, so the price of eerialization doesn't have to be paid when it's not needed.

### Usage
Add to your `Cargo.toml` dependencies:

```toml
tarpc = "0.18.0"
```

The `tarpc::service` attribute expands to a collection of items that form an rpc service.
These generated types make it easy and ergonomic to write servers with less boilerplate.
Simply implement the generated service trait, and you're off to the races!

### Example

For this example, in addition to tarpc, also add two other dependencies to
your `Cargo.toml`:

```toml
futures-preview = "0.3.0-alpha.17"
tokio = "0.2.0-alpha.1"
```

In the following example, we use an in-process channel for communication between
client and server. In real code, you will likely communicate over the network.
For a more real-world example, see [example-service](example-service).

First, let's set up the dependencies and service definition.

```rust
#![feature(async_await)]

use futures::{
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
#[tokio::main]
async fn main() -> io::Result<()> {
    let (client_transport, server_transport) = tarpc::transport::channel::unbounded();

    let server = server::new(server::Config::default())
        // incoming() takes a stream of transports such as would be returned by
        // TcpListener::incoming (but a stream instead of an iterator).
        .incoming(stream::once(future::ready(server_transport)))
        .respond_with(HelloServer.serve());

    tokio::spawn(server);

    // WorldClient is generated by the macro. It has a constructor `new` that takes a config and
    // any Transport as input
    let mut client = WorldClient::new(client::Config::default(), client_transport).spawn()?;

    // The client has an RPC method for each RPC defined in the annotated trait. It takes the same
    // args as defined, with the addition of a Context, which is always the first arg. The Context
    // specifies a deadline and trace information which can be helpful in debugging requests.
    let hello = client.hello(context::current(), "Stim".to_string()).await?;

    println!("{}", hello);

    Ok(())
}
```

### Service Documentation

Use `cargo doc` as you normally would to see the documentation created for all
items expanded by a `service!` invocation.

License: MIT
