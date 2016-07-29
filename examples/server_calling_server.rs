// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(default_type_parameter_fallback, try_from)]

#[macro_use]
extern crate tarpc;
extern crate mio;

use add::{AsyncServiceExt as AddExt, AsyncService as AddService};
use add_one::{AsyncServiceExt as AddOneExt, AsyncService as AddOneService};
use mio::{EventLoop, Handler, Sender, Token};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Mutex;
use std::thread;
use tarpc::{Client, Ctx, RpcId};

pub mod add {
    service! {
        /// Add two ints together.
        rpc add(x: i32, y: i32) -> i32;
    }
}

pub mod add_one {
    service! {
        /// 2 * (x + 1)
        rpc add_one(x: i32) -> i32;
    }
}

struct AddServer;

impl AddService for AddServer {
    fn add(&self, ctx: Ctx<i32>, x: i32, y: i32) {
        ctx.ok(x + y).unwrap();
    }
}

struct AddOneServer {
    client: add::AsyncClient,
    tx: Mutex<Sender<<AddOneServerEvents as Handler>::Message>>,
}

impl AddOneService for AddOneServer {
    fn add_one(&self, ctx1: Ctx<i32>, x: i32) {
        let tx = self.tx.lock().unwrap();
        let tx1 = tx.clone();
        let ctx2 = ctx1.clone();
        let tx2 = tx.clone();
        self.client.add(move |result| tx1.send((ctx1, result)).unwrap(), &x, &1).unwrap();
        self.client.add(move |result| tx2.send((ctx2, result)).unwrap(), &x, &1).unwrap();
    }
}

struct AddOneServerEvents(HashMap<(Token, RpcId), RpcContext>);

struct RpcContext {
    ctx: Ctx<i32>,
    first: tarpc::Result<i32>,
}

impl Handler for AddOneServerEvents {
    type Timeout = ();
    type Message = (Ctx<i32>, tarpc::Result<i32>);

    #[inline]
    fn notify(&mut self, _: &mut EventLoop<Self>, (ctx, result): Self::Message) {
        match self.0.entry((ctx.connection_token(), ctx.request_id())) {
            Entry::Occupied(occupied) => {
                let ctx = occupied.remove();
                let result = ctx.first.and_then(|first| result.map(|second| first + second));
                ctx.ctx.reply(result).unwrap();
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

fn main() {
    let mut event_loop = EventLoop::new().unwrap();
    let tx = event_loop.channel();
    thread::spawn(move || {
        event_loop.run(&mut AddOneServerEvents(HashMap::new())).unwrap();
    });
    let add = AddServer.listen("localhost:0").unwrap();
    let add_client = add::AsyncClient::connect(add.local_addr()).unwrap();
    let add_one = AddOneServer {
        client: add_client,
        tx: Mutex::new(tx),
    };
    let add_one = add_one.listen("localhost:0").unwrap();

    let add_one_client = add_one::SyncClient::connect(add_one.local_addr()).unwrap();
    for i in 0..5 {
        println!("{:?}", add_one_client.add_one(&i).unwrap());
    }
}
