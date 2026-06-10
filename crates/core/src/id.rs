//! Deterministic id generation (spec §6.3).
//!
//! `id(prefix)` => `prefix_` + 24 base62 chars. Checkout sessions use Stripe's
//! longer `cs_test_` + 56 base62 form.

use crate::rng::WorldRng;

/// Generate `"{prefix}_{24 base62}"` from the world RNG.
pub fn id(rng: &mut WorldRng, prefix: &str) -> String {
    format!("{prefix}_{}", rng.fill_base62(24))
}

/// Generate a checkout session id: `"cs_test_{56 base62}"`.
pub fn checkout_session_id(rng: &mut WorldRng) -> String {
    format!("cs_test_{}", rng.fill_base62(56))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_shape() {
        let mut rng = WorldRng::from_seed(1);
        let s = id(&mut rng, "cus");
        assert!(s.starts_with("cus_"));
        assert_eq!(s.len(), "cus_".len() + 24);
    }

    #[test]
    fn checkout_session_shape() {
        let mut rng = WorldRng::from_seed(1);
        let s = checkout_session_id(&mut rng);
        assert!(s.starts_with("cs_test_"));
        assert_eq!(s.len(), "cs_test_".len() + 56);
    }
}
