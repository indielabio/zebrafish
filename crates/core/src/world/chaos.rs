//! Chaos-rule storage (spec §9 Layer 2).
//!
//! The rule *content* (`match`/`action` semantics) is interpreted by the
//! server's chaos engine; core owns persistence, the `times` decrement, and
//! TTL expiry — all against the virtual clock, the only time source (§6.2).

use serde_json::Value;

use crate::bus::Notification;
use crate::error::Result;
use crate::store::ChaosRuleRow;
use crate::world::World;

impl World {
    /// Store a new chaos rule. `times` of `None` means unlimited;
    /// `ttl_seconds` of `None` means no expiry. Returns the stored row.
    pub fn add_chaos_rule(
        &mut self,
        rule: Value,
        times: Option<i64>,
        ttl_seconds: Option<i64>,
    ) -> Result<ChaosRuleRow> {
        let row = ChaosRuleRow {
            id: self.new_id("chaos"),
            rule,
            remaining: times,
            expires_at: ttl_seconds.map(|ttl| self.now() + ttl),
        };
        let world_row = self.world_row()?;
        self.store.transaction(|tx| {
            crate::store::put_chaos_rule(tx, &row)?;
            crate::store::save_world_row(tx, &world_row)
        })?;
        self.bus.publish(Notification::ChaosChanged(row.to_json()));
        Ok(row)
    }

    /// All live (non-exhausted, non-expired) rules in creation order, purging
    /// expired rows as a side effect (spec §9: expired rules auto-delete).
    pub fn list_chaos_rules(&mut self) -> Result<Vec<ChaosRuleRow>> {
        let now = self.now();
        self.store
            .transaction(|tx| crate::store::purge_expired_chaos(tx, now))?;
        self.store.read(|c| crate::store::list_chaos_rules(c, now))
    }

    /// Consume one application of a rule (decrement `times`, auto-deleting on
    /// exhaustion).
    pub fn consume_chaos_rule(&mut self, id: &str) -> Result<()> {
        self.store
            .transaction(|tx| crate::store::consume_chaos_rule(tx, id))?;
        self.bus.publish(Notification::ChaosChanged(
            serde_json::json!({ "id": id, "consumed": true }),
        ));
        Ok(())
    }

    /// Delete one rule. Returns whether it existed.
    pub fn delete_chaos_rule(&mut self, id: &str) -> Result<bool> {
        let removed = self
            .store
            .transaction(|tx| crate::store::delete_chaos_rule(tx, id))?;
        if removed {
            self.bus.publish(Notification::ChaosChanged(
                serde_json::json!({ "id": id, "deleted": true }),
            ));
        }
        Ok(removed)
    }

    /// Delete all rules.
    pub fn clear_chaos_rules(&mut self) -> Result<()> {
        self.store
            .transaction(|tx| crate::store::clear_chaos_rules(tx))?;
        self.bus.publish(Notification::ChaosChanged(
            serde_json::json!({ "cleared": true }),
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::world::World;

    fn world() -> World {
        World::open(":memory:", Some(7)).expect("open world")
    }

    #[test]
    fn chaos_times_decrement_and_auto_delete() {
        let mut w = world();
        let rule = w
            .add_chaos_rule(json!({ "action": { "kind": "error" } }), Some(2), None)
            .unwrap();
        w.consume_chaos_rule(&rule.id).unwrap();
        assert_eq!(w.list_chaos_rules().unwrap().len(), 1);
        w.consume_chaos_rule(&rule.id).unwrap();
        assert!(w.list_chaos_rules().unwrap().is_empty());
    }

    #[test]
    fn chaos_unlimited_rule_survives_consumption() {
        let mut w = world();
        let rule = w
            .add_chaos_rule(json!({ "action": { "kind": "delay" } }), None, None)
            .unwrap();
        for _ in 0..5 {
            w.consume_chaos_rule(&rule.id).unwrap();
        }
        assert_eq!(w.list_chaos_rules().unwrap().len(), 1);
    }

    #[test]
    fn chaos_ttl_expires_against_virtual_clock() {
        let mut w = world();
        w.add_chaos_rule(json!({ "action": { "kind": "error" } }), None, Some(60))
            .unwrap();
        assert_eq!(w.list_chaos_rules().unwrap().len(), 1);
        let target = w.now() + 61;
        w.advance_to(target).unwrap();
        assert!(w.list_chaos_rules().unwrap().is_empty());
    }
}
