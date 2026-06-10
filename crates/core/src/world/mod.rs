//! [`World`] — the single owner of all mutable state (spec §6.1).
//!
//! Every mutation goes through a `World` method so that each write can
//! (a) persist, (b) emit events, (c) advance the RNG deterministically, and
//! (d) notify the bus. The RNG state is committed *inside the same transaction*
//! as the data it produced, so a crash rolls both back together and a restart
//! resumes the exact stream position.

mod advance;
mod mutate;

pub use advance::AdvanceReport;

use tokio::sync::broadcast;

use crate::bus::{Notification, NotificationBus};
use crate::clock::{VirtualClock, wall_clock_now};
use crate::error::Result;
use crate::rng::WorldRng;
use crate::store::{Store, WorldRow, load_world_row, save_world_row};
use crate::{STRIPE_API_VERSION, faker, id};

/// The world: store + virtual clock + seeded RNG + notification bus.
#[derive(Debug)]
pub struct World {
    store: Store,
    clock: VirtualClock,
    rng: WorldRng,
    bus: NotificationBus,
    seed: u64,
    api_version: String,
}

impl World {
    /// Open the world at `path` (or `":memory:"`).
    ///
    /// If the database already holds a `world` row, the clock + RNG are restored
    /// from it (and `seed` is ignored — the persisted run wins). Otherwise a new
    /// world is booted: the clock is set to wall-clock time, the RNG is seeded
    /// from `seed` (or a fresh random seed), and the row is persisted.
    pub fn open(path: &str, seed: Option<u64>) -> Result<Self> {
        let store = Store::open(path)?;
        match store.read(load_world_row)? {
            Some(row) => {
                let rng = WorldRng::from_state_blob(&row.rng_state)?;
                Ok(Self {
                    store,
                    clock: VirtualClock::new(row.now_unix),
                    rng,
                    bus: NotificationBus::new(),
                    seed: row.seed,
                    api_version: row.stripe_api_version,
                })
            }
            None => {
                let seed = seed.unwrap_or_else(faker::random_seed);
                let mut world = Self {
                    store,
                    clock: VirtualClock::new(wall_clock_now()),
                    rng: WorldRng::from_seed(seed),
                    bus: NotificationBus::new(),
                    seed,
                    api_version: STRIPE_API_VERSION.to_string(),
                };
                world.persist_world_row()?;
                Ok(world)
            }
        }
    }

    /// Current virtual time, unix seconds (spec §6.2). The only time source.
    #[must_use]
    pub fn now(&self) -> i64 {
        self.clock.now()
    }

    /// The world seed (surfaced via the `Zebrafish-Seed` header, spec §5).
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The pinned Stripe API version.
    #[must_use]
    pub fn api_version(&self) -> &str {
        &self.api_version
    }

    /// Subscribe to the notification bus (for the dashboard SSE stream).
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.bus.subscribe()
    }

    /// Borrow the RNG so resource/faker code can draw deterministic data.
    pub fn rng(&mut self) -> &mut WorldRng {
        &mut self.rng
    }

    /// Generate a fresh id with the given prefix, advancing the RNG.
    pub fn new_id(&mut self, prefix: &str) -> String {
        id::id(&mut self.rng, prefix)
    }

    /// Generate a fresh checkout-session id (`cs_test_...`).
    pub fn new_checkout_session_id(&mut self) -> String {
        id::checkout_session_id(&mut self.rng)
    }

    /// Read an object's `api_state`, or `None` if absent.
    pub fn get_object(&self, id: &str) -> Result<Option<serde_json::Value>> {
        self.store.read(|c| crate::store::get_object(c, id))
    }

    /// List non-deleted objects of a type, newest first.
    pub fn list_objects(&self, type_: &str) -> Result<Vec<serde_json::Value>> {
        self.store.read(|c| crate::store::query_by_type(c, type_))
    }

    /// Every object as `(id, api_state)` ordered by id — for deterministic
    /// dumps and the dashboard object browser.
    pub fn all_objects(&self) -> Result<Vec<(String, serde_json::Value)>> {
        self.store.read(crate::store::all_objects)
    }

    /// Read an event payload by id.
    pub fn get_event(&self, id: &str) -> Result<Option<serde_json::Value>> {
        self.store.read(|c| crate::store::get_event(c, id))
    }

    /// Flush all object/event/delivery/chaos state, keeping seed + clock + RNG
    /// and any registered webhooks (spec §9, `DELETE /_config/data`).
    pub fn flush_data(&mut self) -> Result<()> {
        self.store.transaction(|tx| crate::store::flush_data(tx))?;
        self.bus.publish(Notification::ClockAdvanced(
            serde_json::json!({ "now": self.clock.now() }),
        ));
        Ok(())
    }

    /// Full reset (spec §9, `POST /_config/reset`): flush data and optionally
    /// reseed the RNG and/or reposition the clock. A reseed restarts the stream
    /// from scratch; an absent `seed` keeps the current seed.
    pub fn reset(&mut self, seed: Option<u64>, clock: Option<i64>) -> Result<()> {
        if let Some(s) = seed {
            self.seed = s;
            self.rng = WorldRng::from_seed(s);
        }
        if let Some(t) = clock {
            self.clock.set(t);
        }
        self.store.transaction(|tx| crate::store::flush_data(tx))?;
        self.persist_world_row()?;
        Ok(())
    }

    /// Snapshot the current clock + seed + RNG state for the `world` row.
    fn world_row(&self) -> Result<WorldRow> {
        Ok(WorldRow {
            now_unix: self.clock.now(),
            seed: self.seed,
            rng_state: self.rng.to_state_blob()?,
            stripe_api_version: self.api_version.clone(),
        })
    }

    /// Persist the world row in its own transaction (boot / reset path).
    fn persist_world_row(&mut self) -> Result<()> {
        let row = self.world_row()?;
        self.store.transaction(|tx| save_world_row(tx, &row))
    }
}
