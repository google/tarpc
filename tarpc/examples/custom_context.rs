// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
#![deny(warnings, unused, dead_code)]

use std::collections::HashMap;
use std::ops::Add;
use std::sync::Arc;
use std::time::{Duration, Instant};
use futures::{SinkExt, TryStreamExt, StreamExt, FutureExt};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tarpc::{client, server::{self, Channel}, trace, ClientMessage, Request, Response, ServerError, Transport};
use tarpc::context::{ExtractContext, SharedContext};
use tarpc::server::request_hook::{AfterRequest, BeforeRequest, RequestHook};
use tarpc::transport::channel::UnboundedChannel;

#[derive(Serialize, Deserialize, Clone)]
struct CustomContext {
  #[serde(with = "absolute_to_relative_time")]
  pub deadline: Instant,
  pub trace_context: trace::Context,
  pub session_id: Option<u64>
}

impl SharedContext for CustomContext {
  fn deadline(&self) -> Instant {
    self.deadline
  }

  fn trace_context(&self) -> trace::Context {
    self.trace_context
  }

  fn set_trace_context(&mut self, trace_context: trace::Context) {
    self.trace_context = trace_context;
  }
}

#[derive(Clone, Debug)]
struct ClientContext {
  pub session_id: Option<u64>,
  pub delay_sending_by_seconds: u32,
}

struct ServerContext {
  pub deadline: Instant,
  pub trace_context: trace::Context,
  pub session_id: Option<u64>,
  pub balance: u64,
}

impl ExtractContext<CustomContext> for ClientContext {
  fn extract(&self) -> CustomContext {
    CustomContext {
      deadline: Instant::now().add(Duration::from_secs(60)),
      trace_context: Default::default(),
      session_id: self.session_id,
    }
  }

  fn update(&mut self, value: CustomContext) {
    self.session_id = value.session_id;
  }
}

impl ExtractContext<CustomContext> for ServerContext {
  fn extract(&self) -> CustomContext {
    CustomContext {
      deadline: self.deadline,
      trace_context: self.trace_context,
      session_id: self.session_id,
    }
  }

  fn update(&mut self, value: CustomContext) {
    self.deadline = value.deadline;
    self.trace_context = value.trace_context;
    self.session_id = value.session_id;
  }
}

/// This is the service definition. It looks a lot like a trait definition.
/// It defines one RPC, hello, which takes one arg, name, and returns a String.
#[tarpc::service(shared_context = "CustomContext")]
pub trait World {
  async fn create_session() -> ();
  async fn increase_balance(credits: u32) -> ();
  async fn hello(name: String) -> Result<String, String>;
}

/// This is the type that implements the generated World trait. It is the business logic
/// and is used to start the server.
#[derive(Clone)]
struct HelloServer;

impl World for HelloServer {
  type Context = ServerContext;

  async fn create_session(self, ctx: &mut Self::Context) -> () {
    ctx.session_id = Some(42);
    ctx.balance = 0;
  }

  async fn increase_balance(self, ctx: &mut Self::Context, credits: u32) -> () {
    ctx.balance = ctx.balance + credits as u64;
  }

  async fn hello(self, ctx: &mut Self::Context, name: String) -> Result<String, String> {
    if ctx.session_id != Some(42) {
      Err("Session not yet initialized!")?
    }

    if ctx.balance == 0 {
      Err("Give me more money")?
    }

    ctx.balance = ctx.balance - 1;

    Ok(format!("Hello, {name}!"))
  }
}

async fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
  tokio::spawn(fut);
}




#[derive(Clone)]
struct SessionHook {
  balances: Arc<Mutex<HashMap<u64, u64>>>
}

impl BeforeRequest<ServerContext, WorldRequest> for SessionHook {
  async fn before(&mut self, ctx: &mut ServerContext, _req: &WorldRequest) -> Result<(), ServerError> {
    if let Some(id) = ctx.session_id {
      let balances = self.balances.lock().await;
      ctx.balance = *balances.get(&id).unwrap_or(&0u64)
    }

    Ok(())
  }
}

impl AfterRequest<ServerContext, WorldResponse> for SessionHook {
  async fn after(&mut self, ctx: &mut ServerContext, resp: &mut Result<WorldResponse, ServerError>) {
    if resp.is_ok() && let Some(id) = ctx.session_id {
      let mut balances = self.balances.lock().await;

      let b = balances.entry(id).or_insert(0);
      *b = ctx.balance;
    }
  }
}




#[tokio::main]
async fn main() -> anyhow::Result<()> {
  let (client_transport, server_transport) = create_channel();

  let client_transport = client_transport.with(|f: ClientMessage<ClientContext, WorldRequest>| async move {

    if let ClientMessage::Request(Request {ref context, ref message, .. }) = f {
      println!("msg = {:?}, ctx = {:?}", message, context);
      tokio::time::sleep(Duration::from_secs(context.delay_sending_by_seconds as u64)).await;
    }

    Ok(f)
  }.boxed());

  let server = server::BaseChannel::with_defaults(server_transport);
  let hook = SessionHook { balances: Arc::new(Mutex::new(HashMap::new())) };
  tokio::spawn(server.execute(HelloServer.serve().before_and_after(hook)).for_each(spawn));

  // WorldClient is generated by the #[tarpc::service] attribute. It has a constructor `new`
  // that takes a config and any Transport as input.
  let client = WorldClient::new(client::Config::default(), client_transport).spawn();

  // The client has an RPC method for each RPC defined in the annotated trait. It takes the same
  // args as defined, with the addition of a Context, which is always the first arg. The Context
  // specifies a deadline and trace information which can be helpful in debugging requests.

  let mut client_context: ClientContext = ClientContext {
    session_id: None,
    delay_sending_by_seconds: 1,
  };

  let hello = client.hello(&mut client_context, "Stan".to_string()).await?;

  assert_eq!(hello, Err("Session not yet initialized!".to_string()));

  let _ = client.create_session(&mut client_context).await?;
  let hello = client.hello(&mut client_context, "Stan".to_string()).await?;
  assert_eq!(hello, Err("Give me more money".to_string()));

  let _ = client.increase_balance(&mut client_context, 2u32).await?;

  let hello = client.hello(&mut client_context, "Stan".to_string()).await?;
  assert_eq!(hello, Ok("Hello, Stan!".to_string()));

  let hello = client.hello(&mut client_context, "Frank".to_string()).await?;
  assert_eq!(hello, Ok("Hello, Frank!".to_string()));

  let hello = client.hello(&mut client_context, "Joshua".to_string()).await?;
  assert_eq!(hello, Err("Give me more money".to_string()));

  Ok(())
}

//*** Helper functions below ***//

mod absolute_to_relative_time {
  pub use serde::{Deserialize, Deserializer, Serialize, Serializer};
  pub use std::time::{Duration, Instant};

  pub fn serialize<S>(deadline: &Instant, serializer: S) -> Result<S::Ok, S::Error>
  where
    S: Serializer,
  {
    let deadline = deadline.duration_since(Instant::now());
    deadline.serialize(serializer)
  }

  pub fn deserialize<'de, D>(deserializer: D) -> Result<Instant, D::Error>
  where
    D: Deserializer<'de>,
  {
    let deadline = Duration::deserialize(deserializer)?;
    Ok(Instant::now() + deadline)
  }
}

fn map_request_context<Ctx, Ctx2, Req>(req: ClientMessage<Ctx, Req>, f: impl FnOnce(Ctx) -> Ctx2) -> ClientMessage<Ctx2, Req> {
  match req {
    ClientMessage::Request(Request { context, id, message}) => ClientMessage::Request(Request { context: f(context), id, message }),
    ClientMessage::Cancel { trace_context, request_id } => ClientMessage::Cancel { trace_context, request_id },
    _ => unimplemented!()
  }
}

fn map_response_context<Ctx, Ctx2, Res>(res: Response<Ctx, Res>, f: impl FnOnce(Ctx) -> Ctx2) -> Response<Ctx2, Res> {
  Response {
    request_id: res.request_id,
    context: f(res.context),
    message: res.message
  }
}

fn create_channel() -> (impl Transport<ClientMessage<ClientContext, WorldRequest>, Response<ClientContext, WorldResponse>>, impl Transport<Response<ServerContext, WorldResponse>, ClientMessage<ServerContext, WorldRequest>>) {
  let (client, server): (UnboundedChannel<Response<CustomContext, WorldResponse>, ClientMessage<CustomContext, WorldRequest>>, UnboundedChannel<ClientMessage<CustomContext, WorldRequest>, Response<CustomContext, WorldResponse>>) = tarpc::transport::channel::unbounded();

  let client = client.with(|m| futures::future::ok(map_request_context(m, |c: ClientContext| c.extract()))).map_ok(|r| map_response_context(r, |c: CustomContext| ClientContext { session_id: c.session_id, delay_sending_by_seconds: 0 }));
  let server = server.with(|r| futures::future::ok(map_response_context(r, |c: ServerContext| c.extract()))).map_ok(|m| map_request_context(m, |c| ServerContext { deadline: c.deadline, trace_context: c.trace_context, session_id: c.session_id, balance: 0 }));

  (client, server)
}