//! Card metadata derived from the entered PAN (spec §6.4).
//!
//! Only non-sensitive derived fields are ever produced or stored — brand,
//! `last4`, expiry, a fingerprint — never the PAN itself (spec §15).

use crate::rng::WorldRng;

/// Map an Issuer Identification Number (the PAN prefix) to a card brand.
/// Matches the magic-card set in spec §9: 4→visa, 5→mastercard, 37→amex.
#[must_use]
pub fn brand_from_pan(pan: &str) -> &'static str {
    let digits: String = pan.chars().filter(char::is_ascii_digit).collect();
    if digits.starts_with('4') {
        "visa"
    } else if digits.starts_with("34") || digits.starts_with("37") {
        "amex"
    } else if digits.starts_with('5') {
        "mastercard"
    } else if digits.starts_with('6') {
        "discover"
    } else {
        "unknown"
    }
}

/// The last four digits of the PAN.
#[must_use]
pub fn last4(pan: &str) -> String {
    let digits: Vec<char> = pan.chars().filter(char::is_ascii_digit).collect();
    let start = digits.len().saturating_sub(4);
    digits[start..].iter().collect()
}

/// A deterministic, opaque card fingerprint (Stripe uses these to dedupe cards).
pub fn card_fingerprint(rng: &mut WorldRng) -> String {
    rng.fill_base62(16)
}

/// A `client_secret` for an object: `"{id}_secret_{24 base62}"` (spec §6.4).
pub fn client_secret(rng: &mut WorldRng, id: &str) -> String {
    format!("{id}_secret_{}", rng.fill_base62(24))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_from_iin() {
        assert_eq!(brand_from_pan("4242424242424242"), "visa");
        assert_eq!(brand_from_pan("5555555555554444"), "mastercard");
        assert_eq!(brand_from_pan("378282246310005"), "amex");
    }

    #[test]
    fn last4_of_pan() {
        assert_eq!(last4("4242 4242 4242 4242"), "4242");
        assert_eq!(last4("378282246310005"), "0005");
    }
}
