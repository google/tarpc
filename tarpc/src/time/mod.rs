// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#[cfg(target_arch="wasm32")]
mod wasm;
#[cfg(target_arch="wasm32")]
pub use wasm::*;

#[cfg(not(target_arch="wasm32"))]
mod non_wasm {
    pub use std::time::{SystemTime, Instant};
    pub use tokio_util::time::delay_queue;
}
#[cfg(not(target_arch="wasm32"))]
pub use non_wasm::*;
