//! Delivery-log access (spec §8): the server's delivery worker records every
//! webhook attempt here; the dashboard and `/_config/deliveries` read it.

use crate::bus::Notification;
use crate::error::Result;
use crate::store::DeliveryRow;
use crate::world::World;

impl World {
    /// Draw a fresh delivery id (`del_...`).
    pub fn new_delivery_id(&mut self) -> String {
        self.new_id("del")
    }

    /// Record one delivery attempt and notify the bus (spec §11: the
    /// Deliveries view is live).
    pub fn record_delivery(&mut self, row: &DeliveryRow) -> Result<()> {
        let world_row = self.world_row()?;
        self.store.transaction(|tx| {
            crate::store::put_delivery(tx, row)?;
            crate::store::save_world_row(tx, &world_row)
        })?;
        self.bus
            .publish(Notification::DeliveryAttempted(row.to_json()));
        Ok(())
    }

    /// All delivery attempts, newest first.
    pub fn list_deliveries(&self) -> Result<Vec<DeliveryRow>> {
        self.store.read(crate::store::list_deliveries)
    }

    /// Delivery attempts for one event, oldest first.
    pub fn deliveries_for_event(&self, event_id: &str) -> Result<Vec<DeliveryRow>> {
        self.store
            .read(|c| crate::store::deliveries_for_event(c, event_id))
    }

    /// The next attempt number for (event, endpoint).
    pub fn next_attempt(&self, event_id: &str, endpoint_id: &str) -> Result<i64> {
        self.store
            .read(|c| crate::store::next_attempt(c, event_id, endpoint_id))
    }
}
