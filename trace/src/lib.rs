// Copyright 2018 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

#[cfg(feature = "serde")]
#[macro_use]
extern crate serde;

use rand::Rng;
use std::{
    fmt::{self, Formatter},
    mem,
};

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Context {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub parent_id: Option<SpanId>,
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct TraceId(u128);

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SpanId(u64);

impl Context {
    pub fn new_root() -> Self {
        let rng = &mut rand::thread_rng();
        Context {
            trace_id: TraceId::random(rng),
            span_id: SpanId::random(rng),
            parent_id: None,
        }
    }
}

impl TraceId {
    pub fn random<R: Rng>(rng: &mut R) -> Self {
        TraceId((rng.next_u64() as u128) << mem::size_of::<u64>() | rng.next_u64() as u128)
    }
}

impl SpanId {
    pub fn random<R: Rng>(rng: &mut R) -> Self {
        SpanId(rng.next_u64())
    }
}

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:02x}", self.0)?;
        Ok(())
    }
}

impl fmt::Display for SpanId {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:02x}", self.0)?;
        Ok(())
    }
}
