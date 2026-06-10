//! The virtual clock — the *only* time source in zebrafish (spec §6.2).
//!
//! This module is the sole place in the workspace permitted to call
//! [`std::time::SystemTime::now`] (enforced by `ci/guardrails.sh`), and even
//! here it is used exactly once: to seed the virtual clock at first boot. After
//! that the clock only ever moves when the operator advances it.

use std::time::{SystemTime, UNIX_EPOCH};

/// Holds the current virtual time as unix seconds. Cheap to copy; the
/// authoritative value is persisted in the `world` table by [`crate::World`].
#[derive(Debug, Clone, Copy)]
pub struct VirtualClock {
    now: i64,
}

impl VirtualClock {
    /// Construct a clock positioned at `now` (unix seconds).
    #[must_use]
    pub fn new(now: i64) -> Self {
        Self { now }
    }

    /// Current virtual time, unix seconds. The only time `core` ever reports.
    #[must_use]
    pub fn now(&self) -> i64 {
        self.now
    }

    /// Move the clock to `t`. Callers persist the new value transactionally.
    pub fn set(&mut self, t: i64) {
        self.now = t;
    }
}

/// Wall-clock unix seconds. The single [`SystemTime::now`] call in the whole
/// workspace; used only to seed [`VirtualClock`] on a world's first boot.
#[must_use]
pub fn wall_clock_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}
