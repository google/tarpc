[![Crates.io][crates-badge]][crates-url]
[![MIT licensed][mit-badge]][mit-url]
[![Build status][gh-actions-badge]][gh-actions-url]
[![Discord chat][discord-badge]][discord-url]

[crates-badge]: https://img.shields.io/crates/v/tarpc.svg
[crates-url]: https://crates.io/crates/tarpc
[mit-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[mit-url]: LICENSE
[gh-actions-badge]: https://github.com/google/tarpc/workflows/Continuous%20integration/badge.svg
[gh-actions-url]: https://github.com/google/tarpc/actions?query=workflow%3A%22Continuous+integration%22
[discord-badge]: https://img.shields.io/discord/647529123996237854.svg?logo=discord&style=flat-square
[discord-url]: https://discord.gg/gXwpdSt

# tarpc

<!-- cargo-sync-readme start -->

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
process, and no context switching between different languages.

Some other features of tarpc:
- Pluggable transport: any type implementing `Stream<Item = Request> + Sink<Response>` can be
  used as a transport to connect the client and server.
- `Send + 'static` optional: if the transport doesn't require it, neither does tarpc!
- Cascading cancellation: dropping a request will send a cancellation message to the server.
  The server will cease any unfinished work on the request, subsequently cancelling any of its
  own requests, repeating for the entire chain of transitive dependencies.
- Configurable deadlines and deadline propagation: request deadlines default to 10s if
  unspecified. The server will automatically cease work when the deadline has passed. Any
  requests sent by the server that use the request context will propagate the request deadline.
  For example, if a server is handling a request with a 10s deadline, does 2s of work, then
  sends a request to another server, that server will see an 8s deadline.
- Distributed tracing: tarpc is instrumented with
  [tracing](https://github.com/tokio-rs/tracing) primitives extended with
  [OpenTelemetry](https://opentelemetry.io/) traces. Using a compatible tracing subscriber like
  [Jaeger](https://github.com/open-telemetry/opentelemetry-rust/tree/main/opentelemetry-jaeger),
  each RPC can be traced through the client, server, and other dependencies downstream of the
  server. Even for applications not connected to a distributed tracing collector, the
  instrumentation can also be ingested by regular loggers like
  [env_logger](https://github.com/env-logger-rs/env_logger/).
- Serde serialization: enabling the `serde1` Cargo feature will make service requests and
  responses `Serialize + Deserialize`. It's entirely optional, though: in-memory transports can
  be used, as well, so the price of serialization doesn't have to be paid when it's not needed.

## Usage
Add to your `Cargo.toml` dependencies:

```toml
tarpc = "0.34"
```

The `tarpc::service` attribute expands to a collection of items that form an rpc service.
These generated types make it easy and ergonomic to write servers with less boilerplate.
Simply implement the generated service trait, and you're off to the races!

## Example

This example uses [tokio](https://tokio.rs), so add the following dependencies to
your `Cargo.toml`:

```toml
anyhow = "1.0"
futures = "0.3"
tarpc = { version = "0.31", features = ["tokio1"] }
tokio = { version = "1.0", features = ["rt-multi-thread", "macros"] }
```

In the following example, we use an in-process channel for communication between
client and server. In real code, you will likely communicate over the network.
For a more real-world example, see [example-service](example-service).

First, let's set up the dependencies and service definition.

```rust
use futures::future::{self, Ready};
use tarpc::{
    client, context,
    server::{self, Channel},
};

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
        future::ready(format!("Hello, {name}!"))
    }
}
```

Lastly let's write our `main` that will start the server. While this example uses an
[in-process channel](transport::channel), tarpc also ships a generic [`serde_transport`]
behind the `serde-transport` feature, with additional [TCP](serde_transport::tcp) functionality
available behind the `tcp` feature.

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (client_transport, server_transport) = tarpc::transport::channel::unbounded();

    let server = server::BaseChannel::with_defaults(server_transport);
    tokio::spawn(server.execute(HelloServer.serve()));

    // WorldClient is generated by the #[tarpc::service] attribute. It has a constructor `new`
    // that takes a config and any Transport as input.
    let client = WorldClient::new(client::Config::default(), client_transport).spawn();

    // The client has an RPC method for each RPC defined in the annotated trait. It takes the same
    // args as defined, with the addition of a Context, which is always the first arg. The Context
    // specifies a deadline and trace information which can be helpful in debugging requests.
    let hello = client.hello(context::current(), "Stim".to_string()).await?;

    println!("{hello}");

    Ok(())
}
```

## Service Documentation

Use `cargo doc` as you normally would to see the documentation created for all
items expanded by a `service!` invocation.

<!-- cargo-sync-readme end -->

License: MIT
