// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::{
    collections::HashMap,
    hash::{BuildHasher, Hash},
    time::{Duration, SystemTime},
};

pub mod deadline_compat;
#[cfg(feature = "serde")]
pub mod serde;

/// Types that can be represented by a [`Duration`].
pub trait AsDuration {
    fn as_duration(&self) -> Duration;
}

impl AsDuration for SystemTime {
    /// Duration of 0 if self is earlier than [`SystemTime::now`].
    fn as_duration(&self) -> Duration {
        self.duration_since(SystemTime::now()).unwrap_or_default()
    }
}

/// Collection compaction; configurable `shrink_to_fit`.
pub trait Compact {
    /// Compacts space if the ratio of length : capacity is less than `usage_ratio_threshold`.
    fn compact(&mut self, usage_ratio_threshold: f64);
}

impl<K, V, H> Compact for HashMap<K, V, H>
where
    K: Eq + Hash,
    H: BuildHasher,
{
    fn compact(&mut self, usage_ratio_threshold: f64) {
        let usage_ratio = self.len() as f64 / self.capacity() as f64;
        if usage_ratio < usage_ratio_threshold {
            self.shrink_to_fit();
        }
    }
}
