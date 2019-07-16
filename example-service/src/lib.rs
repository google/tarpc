// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#![feature(arbitrary_self_types, async_await, proc_macro_hygiene)]

// This is the service definition. It looks a lot like a trait definition.
// It defines one RPC, hello, which takes one arg, name, and returns a String.
tarpc::service! {
    /// Returns a greeting for name.
    rpc hello(#[serde(default = "default_name")] name: String) -> String;
}

fn default_name() -> String {
    "DefaultName".into()
}
