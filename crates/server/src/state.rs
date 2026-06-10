//! Shared application state (spec §4, §5).
//!
//! `World` mutations take `&mut self`, so the world is shared behind a single
//! `std::sync::Mutex` — there is one local user, no async pool. Handlers lock,
//! do their (synchronous) work, and drop the guard before returning; no lock is
//! ever held across an `.await`.

use std::sync::{Arc, Mutex};

use zebrafish_core::World;

use crate::idempotency::IdempotencyStore;

/// Cheaply-cloneable handle to the world and request-scoped stores.
#[derive(Debug, Clone)]
pub struct AppState {
    /// The one and only world.
    pub world: Arc<Mutex<World>>,
    /// Idempotency-Key replay cache.
    pub idempotency: Arc<Mutex<IdempotencyStore>>,
    /// World seed, surfaced via the `Zebrafish-Seed` header.
    pub seed: u64,
    /// Pinned Stripe API version, echoed via the `Stripe-Version` header.
    pub api_version: Arc<str>,
}

impl AppState {
    /// Build state from a freshly-opened world.
    #[must_use]
    pub fn new(world: World) -> Self {
        let seed = world.seed();
        let api_version: Arc<str> = Arc::from(world.api_version());
        Self {
            world: Arc::new(Mutex::new(world)),
            idempotency: Arc::new(Mutex::new(IdempotencyStore::default())),
            seed,
            api_version,
        }
    }

    /// Lock the world. Panics only if a previous holder panicked mid-mutation.
    pub fn world(&self) -> std::sync::MutexGuard<'_, World> {
        self.world.lock().expect("world mutex poisoned")
    }
}
