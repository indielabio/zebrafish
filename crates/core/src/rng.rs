//! The world's single seeded RNG (spec §6.4).
//!
//! All randomness in zebrafish flows from here so that `same seed => identical
//! run`. The ChaCha stream position is serialized into the `world.rng_state`
//! BLOB after every mutation (see [`crate::World`]) so determinism survives a
//! process restart.

use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha20Rng;

use crate::error::Result;

/// Base62 alphabet used for all generated ids. This ordering is part of the
/// id contract — do not reorder.
const B62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Newtype over a ChaCha20 RNG. Resource and faker code draw from this and
/// never construct their own RNG (enforced by `ci/guardrails.sh`).
#[derive(Debug, Clone)]
pub struct WorldRng(ChaCha20Rng);

impl WorldRng {
    /// Seed a fresh RNG from a 64-bit seed.
    #[must_use]
    pub fn from_seed(seed: u64) -> Self {
        Self(ChaCha20Rng::seed_from_u64(seed))
    }

    /// Serialize the full RNG state (seed + stream + word position) for the
    /// `world.rng_state` BLOB. Restoring this resumes the stream exactly.
    pub fn to_state_blob(&self) -> Result<Vec<u8>> {
        Ok(bincode::serde::encode_to_vec(
            &self.0,
            bincode::config::standard(),
        )?)
    }

    /// Restore an RNG previously captured by [`Self::to_state_blob`].
    pub fn from_state_blob(bytes: &[u8]) -> Result<Self> {
        let (rng, _) = bincode::serde::decode_from_slice(bytes, bincode::config::standard())?;
        Ok(Self(rng))
    }

    /// Draw the next `u32` from the stream.
    pub fn next_u32(&mut self) -> u32 {
        self.0.next_u32()
    }

    /// Draw the next `u64` from the stream.
    pub fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }

    /// Uniformly (modulo bias is negligible and deterministic) pick an index in
    /// `0..n`. Panics if `n == 0`.
    pub fn below(&mut self, n: u32) -> u32 {
        assert!(n > 0, "WorldRng::below requires n > 0");
        self.0.next_u32() % n
    }

    /// Generate `n` base62 characters from the stream.
    pub fn fill_base62(&mut self, n: usize) -> String {
        (0..n)
            .map(|_| B62[(self.0.next_u32() % 62) as usize] as char)
            .collect()
    }

    /// Borrow the inner RNG so the `fake` crate can draw from the same stream
    /// (`fake_with_rng` requires a `rand::Rng`).
    pub fn inner(&mut self) -> &mut ChaCha20Rng {
        &mut self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_blob_round_trips_exactly() {
        let mut a = WorldRng::from_seed(7);
        // advance a bit so word_pos is non-zero
        let _ = a.fill_base62(40);
        let blob = a.to_state_blob().unwrap();
        let mut b = WorldRng::from_state_blob(&blob).unwrap();
        // continuing both streams must yield identical output
        assert_eq!(a.fill_base62(64), b.fill_base62(64));
    }

    #[test]
    fn same_seed_same_stream() {
        let mut a = WorldRng::from_seed(42);
        let mut b = WorldRng::from_seed(42);
        assert_eq!(a.fill_base62(48), b.fill_base62(48));
    }
}
