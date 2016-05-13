#![feature(question_mark, default_type_parameter_fallback)]

#[macro_use]
extern crate tarpc;
extern crate mio;

use add::Service as AddService;
use add_one::Service as AddOneService;
use mio::{EventLoop, Handler, Sender, Token};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::thread;

mod add {
    service! {
        rpc add(x: i32, y: i32) -> i32;
    }
}

mod add_one {
    service! {
        /// 2 * (x + 1)
        rpc add_one(x: i32) -> i32;
    }
}

struct AddServer;

impl AddService for AddServer {
    fn add(&mut self, ctx: add::Ctx, x: i32, y: i32) {
        ctx.add(Ok(x + y)).unwrap();
    }
}

struct AddOneServer {
    client: add::Client,
    tx: Sender<<AddOneServerEvents as Handler>::Message>,
}
struct RpcContext {
    ctx: add_one::SendCtx,
    first: tarpc::Result<i32>,
}

struct AddOneServerEvents(HashMap<(Token, u64), RpcContext>);
impl Handler for AddOneServerEvents {
    type Timeout = ();
    type Message = (add_one::SendCtx, tarpc::Result<i32>);

    #[inline]
    fn notify(&mut self, _: &mut EventLoop<Self>, (ctx, result): Self::Message) {
        match self.0.entry((ctx.connection_token(), ctx.request_id())) {
            Entry::Occupied(occupied) => {
                let ctx = occupied.remove();
                let result = ctx.first.and_then(|first| result.map(|second| first + second));
                ctx.ctx.add_one(result).unwrap();
            }
            Entry::Vacant(vacant) => {
                vacant.insert(RpcContext {
                    ctx: ctx,
                    first: result,
                });
            }
        }
    }
}

impl AddOneService for AddOneServer {
    fn add_one(&mut self, ctx: add_one::Ctx, x: i32) {
        let ctx1 = ctx.sendable();
        let tx1 = self.tx.clone();
        let ctx2 = ctx1.clone();
        let tx2 = self.tx.clone();
        self.client.add(move |result| tx1.send((ctx1, result)).unwrap(), &x, &1).unwrap();
        self.client.add(move |result| tx2.send((ctx2, result)).unwrap(), &x, &1).unwrap();
    }
}

fn main() {
    let mut event_loop = EventLoop::new().unwrap();
    let tx = event_loop.channel();
    thread::spawn(move || {
        event_loop.run(&mut AddOneServerEvents(HashMap::new())).unwrap();
    });
    let server_registry = tarpc::server::Dispatcher::spawn().unwrap();
    let client_registry = tarpc::client::Dispatcher::spawn().unwrap();
    let add = AddServer.register("localhost:0", &server_registry).unwrap();
    let add_client = add::Client::register(add.local_addr(), &client_registry).unwrap();
    let add_one = AddOneServer {
        client: add_client,
        tx: tx,
    };
    let add_one = add_one.register("localhost:0", &server_registry).unwrap();

    let add_one_client = add_one::BlockingClient::register(add_one.local_addr(), &client_registry)
                             .unwrap();
    for i in 0..5 {
        println!("{:?}", add_one_client.add_one(&i).unwrap());
    }
}
