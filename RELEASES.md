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

- Enums had _non_exhaustive fields replaced with the #[non_exhaustive] attribute.

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
