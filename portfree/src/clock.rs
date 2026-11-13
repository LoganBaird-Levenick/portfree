//! Time and sleep seam. Tests use a fake clock so observation windows are instant.

use std::time::{Duration, Instant};

pub trait Clock {
    fn now_unix(&self) -> i64;
    /// Monotonic milliseconds.
    fn now_ms(&self) -> u64;
    fn sleep(&self, duration: Duration);
}

pub struct RealClock {
    base: Instant,
}

impl RealClock {
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
        }
    }
}

impl Default for RealClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for RealClock {
    fn now_unix(&self) -> i64 {
        crate::processes::unix_now()
    }

    fn now_ms(&self) -> u64 {
        self.base.elapsed().as_millis() as u64
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
