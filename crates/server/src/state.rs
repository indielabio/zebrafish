//! Shared application state (spec §4, §5).
//!
//! `World` mutations take `&mut self`, so the world is shared behind a single
//! `std::sync::Mutex` — there is one local user, no async pool. Handlers lock,
//! do their (synchronous) work, and drop the guard before returning; no lock is
//! ever held across an `.await`.

use std::sync::{Arc, Mutex};

use zebrafish_core::World;

use crate::idempotency::IdempotencyStore;
use crate::webhooks::DeliveryHandle;
use crate::webhooks::delivery::{WorkerChannels, channels};

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
    /// Handle to the webhook delivery worker (spec §8). Inert until
    /// [`crate::webhooks::spawn_delivery_worker`] runs (the binary does at
    /// boot; in-process test routers may skip it).
    pub delivery: DeliveryHandle,
    /// Receiver halves for the delivery worker, taken exactly once by
    /// `spawn_delivery_worker`.
    worker_channels: Arc<Mutex<Option<WorkerChannels>>>,
}

impl AppState {
    /// Build state from a freshly-opened world, installing the post-commit
    /// event sink that feeds the delivery worker.
    #[must_use]
    pub fn new(mut world: World) -> Self {
        let (event_tx, delivery, worker_channels) = channels();
        world.set_event_sink(event_tx);
        let seed = world.seed();
        let api_version: Arc<str> = Arc::from(world.api_version());
        Self {
            world: Arc::new(Mutex::new(world)),
            idempotency: Arc::new(Mutex::new(IdempotencyStore::default())),
            seed,
            api_version,
            delivery,
            worker_channels: Arc::new(Mutex::new(Some(worker_channels))),
        }
    }

    /// Lock the world. Panics only if a previous holder panicked mid-mutation.
    pub fn world(&self) -> std::sync::MutexGuard<'_, World> {
        self.world.lock().expect("world mutex poisoned")
    }

    /// Take the delivery worker's receiver halves (first caller wins).
    pub(crate) fn take_worker_channels(&self) -> Option<WorkerChannels> {
        self.worker_channels
            .lock()
            .expect("worker channel mutex poisoned")
            .take()
    }
}
