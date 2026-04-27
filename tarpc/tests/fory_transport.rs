// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! End-to-end tests for `serde_transport::fory` over TCP.
//!
//! Uses `String` as both request and response type because built-in types
//! are registered by fory internally and do not need explicit `register`
//! calls, avoiding type-index collisions between the test crate and the tarpc
//! lib crate. See the `fory_envelope` test for a detailed explanation.

#![cfg(all(feature = "serde-transport-fory", feature = "tcp"))]

use fory::Fory;
use futures::StreamExt as _;
use std::sync::Arc;
use tarpc::{
    client,
    context,
    server::{self, Channel},
    serde_transport::fory as fory_transport,
    serde_transport::fory_envelope::{
        ForyClientMessage, ForyRequest, ForyResponse, ForyResult, ForyServerError, ForyTraceContext,
    },
};

// ---------------------------------------------------------------------------
// Helper: build a shared Fory registry with all envelope types registered.
//
// We use String as Req and Resp throughout, so only the envelope wrapper types
// need explicit registration (builtin String is registered internally).
// ---------------------------------------------------------------------------

fn make_fory() -> Arc<Fory> {
    let mut fory = Fory::default();
    fory.register::<ForyTraceContext>(2).unwrap();
    fory.register::<ForyServerError>(3).unwrap();
    fory.register::<ForyResult<String>>(4).unwrap();
    fory.register::<ForyRequest<String>>(5).unwrap();
    fory.register::<ForyResponse<String>>(6).unwrap();
    fory.register::<ForyClientMessage<String>>(7).unwrap();
    Arc::new(fory)
}

// ---------------------------------------------------------------------------
// Raw transport echo test
//
// Exercises the codec directly by sending a single ClientMessage<String> and
// receiving the corresponding Response<String>, without using the tarpc
// service machinery. This validates that:
//   1. ForyEnvelopeCodec can serialize ClientMessage<String>
//   2. The server side can deserialize ClientMessage<String>
//   3. The server can serialize Response<String>
//   4. The client side can deserialize Response<String>
// ---------------------------------------------------------------------------

#[tokio::test]
async fn raw_transport_echo() {
    use futures::SinkExt as _;
    use tarpc::{ClientMessage, Request, Response};

    let fory = make_fory();

    let mut incoming =
        fory_transport::listen::<_, String, String>("127.0.0.1:0", fory.clone())
            .await
            .unwrap();
    let addr = incoming.local_addr();

    // Server: echo the request payload back as a response.
    tokio::spawn(async move {
        if let Some(Ok(mut server_transport)) = incoming.next().await {
            // Receive one ClientMessage<String>
            let msg = futures::StreamExt::next(&mut server_transport).await;
            if let Some(Ok(ClientMessage::Request(req))) = msg {
                let resp = Response {
                    request_id: req.id,
                    message: Ok(format!("echo: {}", req.message)),
                };
                server_transport.send(resp).await.unwrap();
            }
        }
    });

    // Client: connect, send a request, receive the echo.
    let mut client_transport =
        fory_transport::connect::<_, String, String>(addr, fory)
            .await
            .unwrap();

    let ctx = context::current();
    let request = ClientMessage::Request(Request {
        context: ctx,
        id: 1,
        message: "hello".to_string(),
    });

    client_transport.send(request).await.unwrap();

    let response = futures::StreamExt::next(&mut client_transport).await;
    let resp = response.unwrap().unwrap();
    assert_eq!(resp.request_id, 1);
    assert_eq!(resp.message.unwrap(), "echo: hello");
}

// ---------------------------------------------------------------------------
// Hello service end-to-end test via tarpc service macro
//
// Uses tarpc's #[service] attribute to define a Hello RPC service and runs it
// over the fory TCP transport. The generated HelloRequest / HelloResponse are
// enums with a single String payload, but they need ForyObject to cross the
// wire. We define them manually as String-wrapped newtypes and derive
// ForyObject so they register cleanly.
//
// Actually: the generated request/response types from #[tarpc::service] do not
// derive ForyObject — they are serde-only. For a service test we therefore use
// the tarpc channel transport (which is in-memory and serde-based) to drive
// the Hello RPC, and test the fory TCP transport separately in the raw test
// above.
// ---------------------------------------------------------------------------

#[tarpc_plugins::service]
trait Hello {
    async fn hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl Hello for HelloServer {
    async fn hello(self, _: context::Context, name: String) -> String {
        format!("hi {}", name)
    }
}

/// Verifies the Hello service itself works correctly via the in-memory channel
/// transport (which does not require fory registration of generated types).
/// This confirms the service plumbing is correct.
#[tokio::test]
async fn hello_service_in_memory() {
    let (client_transport, server_transport) = tarpc::transport::channel::unbounded();

    tokio::spawn(async move {
        server::BaseChannel::with_defaults(server_transport)
            .execute(HelloServer.serve())
            .for_each(|fut| async move {
                tokio::spawn(fut);
            })
            .await;
    });

    let client = HelloClient::new(client::Config::default(), client_transport).spawn();
    let resp = client.hello(context::current(), "world".to_string()).await.unwrap();
    assert_eq!(resp, "hi world");
}

// ---------------------------------------------------------------------------
// Multiple round-trips: verifies request ID correlation works correctly when
// several messages are sent in sequence over the fory transport.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn raw_transport_multiple_round_trips() {
    use futures::SinkExt as _;
    use tarpc::{ClientMessage, Request, Response};

    let fory = make_fory();

    let mut incoming =
        fory_transport::listen::<_, String, String>("127.0.0.1:0", fory.clone())
            .await
            .unwrap();
    let addr = incoming.local_addr();

    // Server: echo each request back.
    tokio::spawn(async move {
        if let Some(Ok(mut server_transport)) = incoming.next().await {
            loop {
                match futures::StreamExt::next(&mut server_transport).await {
                    Some(Ok(ClientMessage::Request(req))) => {
                        let resp = Response {
                            request_id: req.id,
                            message: Ok(format!("resp:{}", req.message)),
                        };
                        if server_transport.send(resp).await.is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
        }
    });

    let mut client_transport =
        fory_transport::connect::<_, String, String>(addr, fory)
            .await
            .unwrap();

    for i in 0u64..5 {
        let ctx = context::current();
        let request = ClientMessage::Request(Request {
            context: ctx,
            id: i,
            message: format!("msg{}", i),
        });
        client_transport.send(request).await.unwrap();
        let response = futures::StreamExt::next(&mut client_transport).await.unwrap().unwrap();
        assert_eq!(response.request_id, i);
        assert_eq!(response.message.unwrap(), format!("resp:msg{}", i));
    }
}
