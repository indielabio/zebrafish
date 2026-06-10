//! Shared test harness for zebrafish (dev-dependency only).
//!
//! [`WorldBuilder`] is the in-process factory for seeded [`World`]s (WS-A/B).
//! [`CaptureServer`] (a real webhook receiver with `expect_events`) and the
//! out-of-process [`Zebrafish`] binary spawner landed with WS-F; the remaining
//! WS-K harness work (#103) builds on these.

pub mod capture;
pub mod spawn;

pub use capture::{CaptureServer, CapturedDelivery};
pub use spawn::{TEST_API_KEY, Zebrafish, ZebrafishBuilder};

use tempfile::TempDir;
use zebrafish_core::World;

/// Re-exported so harness consumers build worlds against the same core types
/// the server uses.
pub use zebrafish_core;

/// Default port the emulator listens on (spec §5).
pub const DEFAULT_PORT: u16 = 4242;

/// The deterministic seed used by default in tests, so failures are reproducible.
pub const DEFAULT_TEST_SEED: u64 = 42;

/// Builder for seeded in-process [`World`]s used by unit/integration tests.
#[derive(Debug, Clone)]
pub struct WorldBuilder {
    seed: Option<u64>,
}

impl WorldBuilder {
    /// A builder fixed to [`DEFAULT_TEST_SEED`] for reproducibility.
    #[must_use]
    pub fn new() -> Self {
        Self {
            seed: Some(DEFAULT_TEST_SEED),
        }
    }

    /// Use an explicit seed.
    #[must_use]
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Use a fresh random seed (for tests that assert *non*-determinism, or
    /// don't care about reproducibility).
    #[must_use]
    pub fn random_seed(mut self) -> Self {
        self.seed = None;
        self
    }

    /// Build an in-memory world (nothing persisted; no restart possible).
    #[must_use]
    pub fn build_in_memory(&self) -> World {
        World::open(":memory:", self.seed).expect("open in-memory world")
    }

    /// Build a file-backed world inside a fresh temp dir. Returns the world
    /// together with the [`TempWorld`] handle that owns the path; keep the
    /// handle alive (it deletes the dir on drop) and reopen via
    /// [`TempWorld::reopen`] to exercise restart behavior.
    #[must_use]
    pub fn build_temp(&self) -> (World, TempWorld) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir
            .path()
            .join("zebrafish.db")
            .to_str()
            .expect("temp path is valid utf-8")
            .to_string();
        let world = World::open(&path, self.seed).expect("open temp world");
        (world, TempWorld { _dir: dir, path })
    }
}

impl Default for WorldBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Owns a temp database directory for a file-backed test world. Dropping it
/// removes the directory.
#[derive(Debug)]
pub struct TempWorld {
    _dir: TempDir,
    path: String,
}

impl TempWorld {
    /// The on-disk database path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Reopen the same database (simulating a process restart). Restores the
    /// persisted clock and RNG state.
    #[must_use]
    pub fn reopen(&self) -> World {
        World::open(&self.path, None).expect("reopen temp world")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_produces_deterministic_worlds() {
        let a = WorldBuilder::new().build_in_memory();
        let b = WorldBuilder::new().build_in_memory();
        assert_eq!(a.seed(), b.seed());
    }

    #[test]
    fn temp_world_reopen_preserves_seed() {
        let (world, temp) = WorldBuilder::new().seed(99).build_temp();
        let seed = world.seed();
        drop(world);
        assert_eq!(temp.reopen().seed(), seed);
    }
}
