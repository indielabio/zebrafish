//! Realistic price points (spec §6.4).
//!
//! Prices cluster on the endings real merchants use — `x99`, `x00`, `x50` —
//! rather than being uniformly random, so generated catalogs look plausible.

use crate::rng::WorldRng;

/// Currencies Stripe treats as zero-decimal (amount is the whole unit, no
/// cents). Abbreviated to the common ones; extend as needed.
const ZERO_DECIMAL: [&str; 7] = ["jpy", "krw", "vnd", "clp", "isk", "ugx", "xof"];

fn is_zero_decimal(currency: &str) -> bool {
    ZERO_DECIMAL.contains(&currency.to_ascii_lowercase().as_str())
}

/// A weighted, realistic amount in the currency's smallest unit.
///
/// For decimal currencies (usd, eur, …) the cents ending is weighted toward
/// `99`, then `00`, then `50`. Zero-decimal currencies get a whole-unit amount.
pub fn price_amount(rng: &mut WorldRng, currency: &str) -> i64 {
    if is_zero_decimal(currency) {
        // e.g. ¥100 .. ¥9900, in steps of 100
        return i64::from(1 + rng.below(99)) * 100;
    }

    let dollars = i64::from(1 + rng.below(199));
    let ending = match rng.below(10) {
        0..=5 => 99, // x99 — most common
        6..=8 => 0,  // x00
        _ => 50,     // x50
    };
    dollars * 100 + ending
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_amounts_use_realistic_endings() {
        let mut rng = WorldRng::from_seed(9);
        for _ in 0..200 {
            let cents = price_amount(&mut rng, "usd") % 100;
            assert!(matches!(cents, 0 | 50 | 99), "unexpected ending {cents}");
        }
    }

    #[test]
    fn zero_decimal_has_no_cents() {
        let mut rng = WorldRng::from_seed(9);
        let amt = price_amount(&mut rng, "JPY");
        assert_eq!(amt % 100, 0);
    }
}
