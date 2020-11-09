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

#[cfg(feature = "serde1")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde1")))]
pub mod serde;

/// Extension trait for [SystemTimes](SystemTime) in the future, i.e. deadlines.
pub trait TimeUntil {
    /// How much time from now until this time is reached.
    fn time_until(&self) -> Duration;
}

impl TimeUntil for SystemTime {
    fn time_until(&self) -> Duration {
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
        if self.capacity() > 1000 {
            let usage_ratio = self.len() as f64 / self.capacity() as f64;
            if usage_ratio < usage_ratio_threshold {
                self.shrink_to_fit();
            }
        }
    }
}
