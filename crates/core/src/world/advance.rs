//! The clock-advance scheduler (spec §6.2, §7).
//!
//! Advancing walks virtual time forward in event order toward a target:
//! repeatedly find the earliest scheduled subscription renewal/cancellation at
//! or before the target, move the clock *to that moment*, and fire the
//! corresponding cascade trigger — so every cascade runs at exactly the
//! virtual time it is due, in deterministic order (time, then subscription
//! id). When no cascade fixtures are loaded the walk degrades to simply
//! moving the clock (the pre-WS-E behavior).

use serde::Serialize;
use serde_json::{Value, json};

use crate::bus::Notification;
use crate::error::{CoreError, Result};
use crate::event::RequestCtx;
use crate::store::save_world_row;
use crate::world::World;

/// The outcome of an advance: the new time and the ids of events emitted by
/// cascades fired while walking there.
#[derive(Debug, Clone, Serialize)]
pub struct AdvanceReport {
    /// The virtual time after advancing.
    pub now: i64,
    /// Ids of events emitted during the walk, in emission order.
    pub events_emitted: Vec<String>,
}

/// One pending lifecycle moment: subscription `sub_id` is due for `trigger`
/// at virtual time `at`.
struct Due {
    at: i64,
    sub_id: String,
    trigger: &'static str,
    subscription: Value,
}

impl World {
    /// Advance the virtual clock to `target` (a no-op if `target` is in the
    /// past or present), firing every scheduled renewal/cancellation cascade
    /// on the way. Returns the events emitted while walking there.
    pub fn advance_to(&mut self, target: i64) -> Result<AdvanceReport> {
        let mut events_emitted = Vec::new();

        let library = self.cascade_library();
        if library.has_trigger("subscription.renew") || library.has_trigger("subscription.cancel") {
            while let Some(due) = self.next_scheduled(target)? {
                if due.at > self.now() {
                    self.set_clock(due.at)?;
                }
                let ctx = self.lifecycle_context(due.trigger, due.subscription)?;
                // Scheduler-fired events carry no originating request (spec §8).
                match self.run_trigger(due.trigger, ctx, &RequestCtx::default())? {
                    Some(outcome) => {
                        events_emitted.extend(outcome.events.into_iter().map(|e| e.id));
                    }
                    // The due trigger has no fixture packaged — stop scheduling
                    // rather than spin; the clock still reaches the target.
                    None => break,
                }
                // Non-progress guard: a cascade that neither advances the
                // period nor changes status would fire forever — fail loudly.
                if let Some(next) = self.next_scheduled(target)?
                    && next.at == due.at
                    && next.sub_id == due.sub_id
                    && next.trigger == due.trigger
                {
                    return Err(CoreError::Cascade(format!(
                        "cascade for '{}' did not advance the schedule of {} \
                         (still due at {})",
                        due.trigger, due.sub_id, due.at
                    )));
                }
            }
        }

        if target > self.now() {
            self.set_clock(target)?;
        }

        Ok(AdvanceReport {
            now: self.now(),
            events_emitted,
        })
    }

    /// Build the trigger context for a scheduled lifecycle moment. Renewals
    /// resolve the default payment method's card outcome (spec §9: one
    /// `card_outcome` for checkout, scheduler, and `when` clauses) — from the
    /// stored `last4`, since PANs are never persisted (spec §15) — and carry
    /// the payment method so fixtures can reference it.
    fn lifecycle_context(&self, trigger: &str, subscription: Value) -> Result<Value> {
        let mut ctx = json!({ "subscription": subscription });
        if trigger == "subscription.renew" {
            let pm = ctx["subscription"]
                .get("default_payment_method")
                .and_then(Value::as_str)
                .and_then(|id| self.get_live_object(id).ok().flatten());
            let last4 = pm
                .as_ref()
                .and_then(|p| p.pointer("/card/last4"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let outcome =
                crate::cards::outcome_from_last4(last4, crate::cards::ChargeContext::OffSession);
            ctx["card"] = outcome.to_context();
            if let Some(pm) = pm {
                ctx["payment_method"] = pm;
            }
        }
        Ok(ctx)
    }

    /// The earliest scheduled renewal/cancellation at or before `target`
    /// (ties broken by subscription id, for determinism). Scans subscriptions
    /// in Rust — local worlds hold tens of subscriptions, not millions.
    fn next_scheduled(&self, target: i64) -> Result<Option<Due>> {
        let mut best: Option<Due> = None;

        for subscription in self.list_objects("subscription")? {
            let status = subscription
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("");
            if matches!(status, "canceled" | "incomplete_expired") {
                continue;
            }
            let Some(period_end) = subscription
                .pointer("/items/data")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| item.get("current_period_end").and_then(Value::as_i64))
                .min()
            else {
                continue;
            };

            let cancel_at = subscription.get("cancel_at").and_then(Value::as_i64);
            let cancel_at_period_end = subscription
                .get("cancel_at_period_end")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let (at, trigger) = match cancel_at {
                Some(c) if c <= period_end => (c, "subscription.cancel"),
                _ if cancel_at_period_end => (period_end, "subscription.cancel"),
                _ => (period_end, "subscription.renew"),
            };
            if at > target {
                continue;
            }

            let sub_id = subscription
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let earlier = best
                .as_ref()
                .is_none_or(|b| (at, &sub_id) < (b.at, &b.sub_id));
            if earlier {
                best = Some(Due {
                    at,
                    sub_id,
                    trigger,
                    subscription,
                });
            }
        }
        Ok(best)
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
