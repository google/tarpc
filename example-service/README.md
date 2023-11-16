# Example

Example service to demonstrate how to set up `tarpc` with [Jaeger](https://www.jaegertracing.io). To see traces Jaeger, run the following with `RUST_LOG=trace`.

## Server

```bash
cargo run --bin server -- --port 50051
```

## Client

```bash
cargo run --bin client -- --server-addr "[::1]:50051" --name "Bob"
```
