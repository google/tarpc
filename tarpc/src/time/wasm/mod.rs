use std::pin::Pin;
use std::time::Duration;

pub mod delay_queue;
mod wheel;

use futures::Future;
pub use wasm_timer::{Instant, SystemTime};

use wasm_timer::Delay;

#[derive(Debug)]
pub struct Sleep(Delay, Instant);

impl Sleep {
    pub fn reset(self: Pin<&mut Self>, deadline: Instant) {
        let this = self.get_mut();
        this.1 = deadline;
        this.0.reset_at(deadline);
    }

    pub fn deadline(&self) -> Instant {
        self.1
    }
}

impl Future for Sleep {
    type Output = std::io::Result<()>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        Future::poll(Pin::new(&mut self.get_mut().0), cx)
    }
}

pub fn sleep_until(deadline: Instant) -> Sleep {
    Sleep(Delay::new_at(deadline), deadline)
}

// ===== Internal utils =====

enum Round {
    Up,
    Down,
}

/// Convert a `Duration` to milliseconds, rounding up and saturating at
/// `u64::MAX`.
///
/// The saturating is fine because `u64::MAX` milliseconds are still many
/// million years.
#[inline]
fn ms(duration: Duration, round: Round) -> u64 {
    const NANOS_PER_MILLI: u32 = 1_000_000;
    const MILLIS_PER_SEC: u64 = 1_000;

    // Round up.
    let millis = match round {
        Round::Up => (duration.subsec_nanos() + NANOS_PER_MILLI - 1) / NANOS_PER_MILLI,
        Round::Down => duration.subsec_millis(),
    };

    duration
        .as_secs()
        .saturating_mul(MILLIS_PER_SEC)
        .saturating_add(u64::from(millis))
}
