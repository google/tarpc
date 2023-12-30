## 0.34.0 (2023-12-29)

### Breaking Changes

- `#[tarpc::server]` is no more! Service traits now use async fns.
- `Channel::execute` no longer spawns request handlers. Async-fn-in-traits makes it impossible to
  add a Send bound to the future returned by `Serve::serve`. Instead, `Channel::execute` returns a
  stream of futures, where each future is a request handler. To achieve the former behavior:
  ```rust
  channel.execute(server.serve())
         .for_each(|rpc| { tokio::spawn(rpc); })
  ```

### New Features

- Request hooks are added to the serve trait, so that it's easy to hook in cross-cutting
  functionality look throttling, authorization, etc.
- The Client trait is back! This makes it possible to hook in generic client functionality like load
  balancing, retries, etc.

## 0.33.0 (2023-04-01)

### Breaking Changes

Opentelemetry dependency version increased to 0.18.

## 0.32.0 (2023-03-24)

### Breaking Changes

- As part of a fix to return more channel errors in RPC results, a few error types have changed:

  0. `client::RpcError::Disconnected` was split into the following errors:
    - Shutdown: the client was shutdown, either intentionally or due to an error. If due to an
      error, pending RPCs should see the more specific errors below.
    - Send: an RPC message failed to send over the transport. Only the RPC that failed to be sent
      will see this error.
    - Receive: a fatal error occurred while receiving from the transport. All in-flight RPCs will
      receive this error.
  0. `client::ChannelError` and `server::ChannelError` are unified in `tarpc::ChannelError`.
     Previously, server transport errors would not indicate during which activity the transport
     error occurred. Now, just like the client already was, it will be specific: reading, readying,
     sending, flushing, or closing.

## 0.31.0 (2022-11-03)

### New Features

This release adds Unix Domain Sockets to the `serde_transport` module.
To use it, enable the "unix" feature. See the docs for more information.

## 0.30.0 (2022-08-12)

### Breaking Changes

- Some types that impl Future are now annotated with `#[must_use]`. Code that previously created
  these types but did not use them will now receive a warning. Code that disallows warnings will
  receive a compilation error.

### Fixes

- Servers will more reliably clean up request state for requests with long deadlines when response
processing is aborted without sending a response.

### Other Changes

- `TrackedRequest` now contains a response guard that can be used to ensure state cleanup for
  aborted requests. (This was already handled automatically by `InFlightRequests`).
- When the feature serde-transport is enabled, the crate tokio_serde is now re-exported.

## 0.29.0 (2022-05-26)

### Breaking Changes

`Context.deadline` is now serialized as a Duration. This prevents clock skew from affecting deadline
behavior. For more details see https://github.com/google/tarpc/pull/367 and its [related
issue](https://github.com/google/tarpc/issues/366).

## 0.28.0 (2022-04-06)

### Breaking Changes

- The minimum supported Rust version has increased to 1.58.0.
- The version of opentelemetry depended on by tarpc has increased to 0.17.0.

## 0.27.2 (2021-10-08)

### Fixes

Clients will now close their transport before dropping it. An attempt at a clean shutdown can help
the server drop its connections more quickly.

## 0.27.1 (2021-09-22)

### Breaking Changes

#### RPC error type is changing

RPC return types are changing from `Result<Response, io::Error>` to `Result<Response,
tarpc::client::RpcError>`.

Becaue tarpc is a library, not an application, it should strive to
use structured errors in its API so that users have maximal flexibility
in how they handle errors. io::Error makes that hard, because it is a
kitchen-sink error type.

RPCs in particular only have 3 classes of errors:

- The connection breaks.
- The request expires.
- The server decides not to process the request.

RPC responses can also contain application-specific errors, but from the
perspective of the RPC library, those are opaque to the framework, classified
as successful responsees.

### Open Telemetry

The Opentelemetry dependency is updated to version 0.16.x.

## 0.27.0 (2021-09-22)

This version was yanked due to tarpc-plugins version mismatches.


## 0.26.0 (2021-04-14)

### New Features

#### Tracing

tarpc is now instrumented with tracing primitives extended with
OpenTelemetry traces. Using a compatible tracing-opentelemetry
subscriber like Jaeger, each RPC can be traced through the client,
server, amd other dependencies downstream of the server. Even for
applications not connected to a distributed tracing collector, the
instrumentation can also be ingested by regular loggers like env_logger.

### Breaking Changes

#### Logging

Logged events are now structured using tracing. For applications using a
logger and not a tracing subscriber, these logs may look different or
contain information in a less consumable manner. The easiest solution is
to add a tracing subscriber that logs to stdout, such as
tracing_subscriber::fmt.

####  Context

- Context no longer has parent_span, which was actually never needed,
  because the context sent in an RPC is inherently the parent context.
  For purposes of distributed tracing, the client side of the RPC has all
  necessary information to link the span to its parent; the server side
  need do nothing more than export the (trace ID, span ID) tuple.
- Context has a new field, SamplingDecision, which has two variants,
  Sampled and Unsampled. This field can be used by downstream systems to
  determine whether a trace needs to be exported. If the parent span is
  sampled, the expectation is that all child spans be exported, as well;
  to do otherwise could result in lossy traces being exported. Note that
  if an Openetelemetry tracing subscriber is not installed, the fallback
  context will still be used, but the Context's sampling decision will
  always be inherited by the parent Context's sampling decision.
- Context::scope has been removed. Context propagation is now done via
  tracing's task-local spans. Spans can be propagated across tasks via
  Span::in_scope. When a service receives a request, it attaches an
  Opentelemetry context to the local Span created before request handling,
  and this context contains the request deadline. This span-local deadline
  is retrieved by Context::current, but it cannot be modified so that
  future Context::current calls contain a different deadline. However, the
  deadline in the context passed into an RPC call will override it, so
  users can retrieve the current context and then modify the deadline
  field, as has been historically possible.
- Context propgation precedence changes: when an RPC is initiated, the
  current Span's Opentelemetry context takes precedence over the trace
  context passed into the RPC method. If there is no current Span, then
  the trace context argument is used as it has been historically. Note
  that Opentelemetry context propagation requires an Opentelemetry
  tracing subscriber to be installed.

#### Server

- The server::Channel trait now has an additional required associated
  type and method which returns the underlying transport. This makes it
  more ergonomic for users to retrieve transport-specific information,
  like IP Address. BaseChannel implements Channel::transport by returning
  the underlying transport, and channel decorators like Throttler just
  delegate to the Channel::transport method of the wrapped channel.

#### Client

- NewClient::spawn no longer returns a result, as spawn can't fail.

### References

1. https://github.com/tokio-rs/tracing
2. https://opentelemetry.io
3. https://github.com/open-telemetry/opentelemetry-rust/tree/main/opentelemetry-jaeger
4. https://github.com/env-logger-rs/env_logger

## 0.25.0 (2021-03-10)

### Breaking Changes

#### Major server module refactoring

1. Renames

Some of the items in this module were renamed to be less generic:

- Handler => Incoming
- ClientHandler => Requests
- ResponseHandler => InFlightRequest
- Channel::{respond_with => requests}

In the case of Handler: handler of *what*? Now it's a bit clearer that this is a stream of Channels
(aka *incoming* connections).

Similarly, ClientHandler was a stream of requests over a single connection. Hopefully Requests
better reflects that.

ResponseHandler was renamed InFlightRequest because it no longer contains the serving function.
Instead, it is just the request, plus the response channel and an abort hook. As a result of this,
Channel::respond_with underwent a big change: it used to take the serving function and return a
ClientHandler; now it has been renamed Channel::requests and does not take any args.

2. Execute methods

All methods thats actually result in responses being generated have been consolidated into methods
named `execute`:

- InFlightRequest::execute returns a future that completes when a response has been generated and
  sent to the server Channel.
- Requests::execute automatically spawns response handlers for all requests over a single channel.
- Channel::execute is a convenience for `channel.requests().execute()`.
- Incoming::execute automatically spawns response handlers for all requests over all channels.

3. Removal of Server.

server::Server was removed, as it provided no value over the Incoming/Channel abstractions.
Additionally, server::new was removed, since it just returned a Server.

#### Client RPC methods now take &self

This required the breaking change of removing the Client trait. The intent of the Client trait was
to facilitate the decorator pattern by allowing users to create their own Clients that added
behavior on top of the base client. Unfortunately, this trait had become a maintenance burden,
consistently causing issues with lifetimes and the lack of generic associated types. Specifically,
it meant that Client impls could not use async fns, which is no longer tenable today, with channel
libraries moving to async fns.

#### Servers no longer send deadline-exceed responses.

The deadline-exceeded response was largely redundant, because the client
shouldn't normally be waiting for such a response, anyway -- the normal
client will automatically remove the in-flight request when it reaches
the deadline.

This also allows for internalizing the expiration+cleanup logic entirely
within BaseChannel, without having it leak into the Channel trait and
requiring action taken by the Requests struct.

#### Clients no longer send cancel messages when the request deadline is exceeded.

The server already knows when the request deadline was exceeded, so the client didn't need to inform
it.

### Fixes

- When a channel is dropped, all in-flight requests for that channel are now aborted.

## 0.24.1 (2020-12-28)

### Breaking Changes

Upgrades tokio to 1.0.

## 0.24.0 (2020-12-28)

This release was yanked.

## 0.23.0 (2020-10-19)

### Breaking Changes

Upgrades tokio to 0.3.

## 0.22.0 (2020-08-02)

This release adds some flexibility and consistency to `serde_transport`, with one new feature and
one small breaking change.

### New Features

`serde_transport::tcp` now exposes framing configuration on `connect()` and `listen()`. This is
useful if, for instance, you want to send requests or responses that are larger than the maximum
payload allowed by default:

```rust
let mut transport = tarpc::serde_transport::tcp::connect(server_addr, Json::default);
transport.config_mut().max_frame_length(4294967296);
let mut client = MyClient::new(client::Config::default(), transport.await?).spawn()?;
```

### Breaking Changes

The codec argument to `serde_transport::tcp::connect` changed from a Codec to impl Fn() -> Codec,
to be consistent with `serde_transport::tcp::listen`. While only one Codec is needed, more than one
person has been tripped up by the inconsistency between `connect` and `listen`. Unfortunately, the
compiler errors are not much help in this case, so it was decided to simply do the more intuitive
thing so that the compiler doesn't need to step in in the first place.


## 0.21.1 (2020-08-02)

### New Features

#### #[tarpc::server] diagnostics

When a service impl uses #[tarpc::server], only `async fn`s are re-written. This can lead to
confusing compiler errors about missing associated types:

```
error: not all trait items implemented, missing: `HelloFut`
 --> $DIR/tarpc_server_missing_async.rs:9:1
  |
9 | impl World for HelloServer {
  | ^^^^
```

The proc macro now provides better diagnostics for this case:

```
error: not all trait items implemented, missing: `HelloFut`
 --> $DIR/tarpc_server_missing_async.rs:9:1
  |
9 | impl World for HelloServer {
  | ^^^^

error: hint: `#[tarpc::server]` only rewrites async fns, and `fn hello` is not async
  --> $DIR/tarpc_server_missing_async.rs:10:5
   |
10 |     fn hello(name: String) ->  String {
   |     ^^
```

### Bug Fixes

#### Fixed client hanging when server shuts down

Previously, clients would ignore when the read half of the transport was closed, continuing to
write requests. This didn't make much sense, because without the ability to receive responses,
clients have no way to know if requests were actually processed by the server. It basically just
led to clients that would hang for a few seconds before shutting down. This has now been
corrected: clients will immediately shut down when the read-half of the transport is closed.

#### More docs.rs documentation

Previously, docs.rs only documented items enabled by default, notably leaving out documentation
for tokio and serde features. This has now been corrected: docs.rs should have documentation
for all optional features.

## 0.21.0 (2020-06-26)

### New Features

A new proc macro, `#[tarpc::server]` was added! This enables service impls to elide the boilerplate
of specifying associated types for each RPC. With the ubiquity of async-await, most code won't have
nameable futures and will just be boxing the return type anyway. This macro does that for you.

### Breaking Changes

- Enums had `_non_exhaustive` fields replaced with the #[non_exhaustive] attribute.

### Bug Fixes

- https://github.com/google/tarpc/issues/304

  A race condition in code that limits number of connections per client caused occasional panics.

- https://github.com/google/tarpc/pull/295

  Made request timeouts account for time spent in the outbound buffer. Previously, a large outbound
  queue would lead to requests not timing out correctly.

## 0.20.0 (2019-12-11)

### Breaking Changes

1. tarpc has updated its tokio dependency to the latest 0.2 version.
2. The tarpc crates have been unified into just `tarpc`, with new Cargo features to enable
   functionality.
   - The bincode-transport and json-transport crates are deprecated and superseded by
     the `serde_transport` module, which unifies much of the logic present in both crates.

## 0.13.0 (2018-10-16)

### Breaking Changes

Version 0.13 marks a significant departure from previous versions of tarpc. The
API has changed significantly. The tokio-proto crate has been torn out and
replaced with a homegrown rpc framework. Additionally, the crate has been
modularized, so that the tarpc crate itself contains only the macro code.

### New Crates

- crate rpc contains the core client/server request-response framework, as well as a transport trait.
- crate bincode-transport implements a transport that works almost exactly as tarpc works today (not to say it's wire-compatible).
- crate trace has some foundational types for tracing. This isn't really fleshed out yet, but it's useful for in-process log tracing, at least.

All crates are now at the top level. e.g. tarpc-plugins is now tarpc/plugins rather than tarpc/src/plugins. tarpc itself is now a *very* small code surface, as most functionality has been moved into the other more granular crates.

### New Features
- deadlines: all requests specify a deadline, and a server will stop processing a response when past its deadline.
- client cancellation propagation: when a client drops a request, the client sends a message to the server informing it to cancel its response. This means cancellations can propagate across multiple server hops.
- trace context stuff as mentioned above
- more server configuration for total connection limits, per-connection request limits, etc.

### Removals
- no more shutdown handle.  I left it out for now because of time and not being sure what the right solution is.
- all async now, no blocking stub or server interface. This helps with maintainability, and async/await makes async code much more usable. The service trait is thusly renamed Service, and the client is renamed Client.
- no built-in transport. Tarpc is now transport agnostic (see bincode-transport for transitioning existing uses).
- going along with the previous bullet, no preferred transport means no TLS support at this time. We could make a tls transport or make bincode-transport compatible with TLS.
- a lot of examples were removed because I couldn't keep up with maintaining all of them. Hopefully the ones I kept are still illustrative.
- no more plugins!

## 0.10.0 (2018-04-08)

### Breaking Changes
Fixed rustc breakage in tarpc-plugins. These changes require a recent version of rustc.

## 0.10.0 (2018-03-26)

### Breaking Changes
Updates bincode to version 1.0.

## 0.9.0 (2017-09-17)

### Breaking Changes
Updates tarpc to use tarpc-plugins 0.2.

## 0.8.0 (2017-05-05)

### Breaking Changes
This release updates tarpc to use serde 1.0.
As such, users must also update to use serde 1.0.
The serde 1.0 [release notes](https://github.com/serde-rs/serde/releases/tag/v1.0.0)
detail migration paths.

## 0.7.3 (2017-04-26)

This release removes the `Sync` bound on RPC args for both sync and future
clients. No breaking changes.

## 0.7.2 (2017-04-22)

### Breaking Changes
This release updates tarpc-plugins to work with rustc master. Thus, older
versions of rustc are no longer supported. We chose a minor version bump
because it is still source-compatible with existing code using tarpc.

## 0.7.1 (2017-03-31)

This release was purely doc fixes. No breaking changes.

## 0.7 (2017-03-31)

### Breaking Changes
This release is a complete overhaul to build tarpc on top of the tokio stack.
It's safe to assume that everything broke with this release.

Two traits are now generated by the macro, `FutureService` and `SyncService`.
`SyncService` is the successor to the original `Service` trait. It uses a configurable
thread pool to serve requests. `FutureService`, as the name implies, uses futures
to serve requests and is single-threaded by default.

The easiest way to upgrade from a 0.6 service impl is to `impl SyncService for MyService`.
For more complete information, see the readme and the examples directory.

## 0.6 (2016-08-07)

### Breaking Changes
* Updated serde to 0.8. Requires dependents to update as well.

## 0.5 (2016-04-24)

### Breaking Changes
0.5 adds support for arbitrary transports via the
[`Transport`](tarpc/src/transport/mod.rs#L7) trait.
Out of the box tarpc provides implementations for:

* Tcp, for types `impl`ing `ToSocketAddrs`.
* Unix sockets via the `UnixTransport` type.

This was a breaking change: `handler.local_addr()` was renamed
`handler.dialer()`.

## 0.4 (2016-04-02)

### Breaking Changes
* Updated to the latest version of serde, 0.7.0. Because tarpc exposes serde in
  its API, this forces downstream code to update to the latest version of
  serde, as well.

## 0.3 (2016-02-20)

### Breaking Changes
* The timeout arg to `serve` was replaced with a `Config` struct, which
  currently only contains one field, but will be expanded in the future
  to allow configuring serialization protocol, and other things.
* `serve` was changed to be a default method on the generated `Service` traits,
  and it was renamed `spawn_with_config`. A second `default fn` was added:
  `spawn`, which takes no `Config` arg.

### Other Changes
* Expanded items will no longer generate unused warnings.
