//! The clock-advance scheduler (spec §6.2).
//!
//! Advancing walks virtual time forward in event order toward a target. WS-D
//! plugs renewal/cancel cascades into the per-step hook; for now the walk just
//! moves the clock (no cascades are scheduled yet), persisting clock + RNG state
//! transactionally and notifying the bus each step.

use serde::Serialize;
use serde_json::json;

use crate::bus::Notification;
use crate::error::Result;
use crate::store::save_world_row;
use crate::world::World;

/// The outcome of an advance: the new time and the ids of events emitted while
/// walking there (empty until cascades land in WS-D).
#[derive(Debug, Clone, Serialize)]
pub struct AdvanceReport {
    /// The virtual time after advancing.
    pub now: i64,
    /// Ids of events emitted during the walk, in emission order.
    pub events_emitted: Vec<String>,
}

impl World {
    /// Advance the virtual clock to `target` (a no-op if `target` is in the
    /// past or present). Returns the events emitted while walking there.
    pub fn advance_to(&mut self, target: i64) -> Result<AdvanceReport> {
        let events_emitted = Vec::new();

        // TODO(WS-D): before reaching `target`, find the earliest scheduled
        // renewal/cancel and run its cascade at that timestamp, looping until
        // no scheduled event remains <= target.

        if target > self.now() {
            self.set_clock(target)?;
        }

        Ok(AdvanceReport {
            now: self.now(),
            events_emitted,
        })
    }

    /// Move the clock to `t` and persist clock + RNG state transactionally.
    fn set_clock(&mut self, t: i64) -> Result<()> {
        self.clock.set(t);
        let row = self.world_row()?;
        self.store.transaction(|tx| save_world_row(tx, &row))?;
        self.bus
            .publish(Notification::ClockAdvanced(json!({ "now": t })));
        Ok(())
    }
}
