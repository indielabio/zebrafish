//! The one place entropy enters the system (spec §6.4, guardrail B).
//!
//! Used only to choose a default seed when neither `ZEBRAFISH_SEED` nor an
//! explicit seed is supplied. Once chosen, the seed is logged and persisted, so
//! the run is still fully reproducible from that point on.

use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha20Rng;

/// Draw a fresh 64-bit seed from the operating system's entropy source.
#[must_use]
pub fn random_seed() -> u64 {
    // `from_os_rng` is OS-seeded; calling it here (and only here) keeps the
    // entropy boundary inside `faker`.
    ChaCha20Rng::from_os_rng().next_u64()
}
