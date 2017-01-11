// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#![feature(conservative_impl_trait, plugin)]
#![plugin(tarpc_plugins)]

extern crate futures;
#[macro_use]
extern crate tarpc;
#[macro_use]
extern crate serde_derive;

use std::error::Error;
use std::fmt;
use tarpc::sync::Connect;
use tarpc::{ClientConfig, ServerConfig};

service! {
    rpc hello(name: String) -> String | NoNameGiven;
}

#[derive(Debug, Deserialize, Serialize)]
pub struct NoNameGiven;

impl fmt::Display for NoNameGiven {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}

impl Error for NoNameGiven {
    fn description(&self) -> &str {
        r#"The empty String, "", is not a valid argument to rpc `hello`."#
    }
}

#[derive(Clone)]
struct HelloServer;

impl SyncService for HelloServer {
    fn hello(&self, name: String) -> Result<String, NoNameGiven> {
        if name == "" {
            Err(NoNameGiven)
        } else {
            Ok(format!("Hello, {}!", name))
        }
    }
}

fn main() {
    let addr = HelloServer.listen("localhost:10000", ServerConfig::new_tcp()).unwrap();
    let client = SyncClient::connect(addr, ClientConfig::new_tcp()).unwrap();
    println!("{}", client.hello("Mom".to_string()).unwrap());
    println!("{}", client.hello("".to_string()).unwrap_err());
}
