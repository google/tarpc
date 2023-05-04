// Copyright 2022 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Hooks for horizontal functionality that can run either before or after a request is executed.

/// A request hook that runs before a request is executed.
mod before;

/// A request hook that runs after a request is completed.
mod after;

/// A request hook that runs both before a request is executed and after it is completed.
mod before_and_after;

pub use {
    after::{AfterRequest, AfterRequestHook},
    before::{BeforeRequest, BeforeRequestHook},
    before_and_after::BeforeAndAfterRequestHook,
};
