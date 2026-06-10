//! The `Stripe-Signature` header (spec §8) — byte-exact with Stripe.
//!
//! `t=<unix>,v1=<hex hmac-sha256 over "{t}.{raw_body}">` with the endpoint's
//! `whsec_...` secret as the HMAC key (the whole string including the prefix,
//! exactly as Stripe's SDK verifiers expect). `t` is the virtual clock.
//! Verified against the real stripe-node and stripe-go verifiers in CI — a
//! release blocker (spec §16.3).

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Build a `Stripe-Signature` header value for `payload` signed at virtual
/// time `t` with `secret`.
#[must_use]
pub fn stripe_signature(secret: &str, t: i64, payload: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("hmac-sha256 accepts keys of any length");
    mac.update(t.to_string().as_bytes());
    mac.update(b".");
    mac.update(payload);
    format!("t={t},v1={}", hex(&mac.finalize().into_bytes()))
}

/// Lowercase hex, no separators (the `v1=` encoding).
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vector precomputed independently (python `hmac` stdlib), so the format
    /// string and key handling are pinned, not self-verified.
    #[test]
    fn signature_matches_independent_vector() {
        let sig = stripe_signature(
            "whsec_zebrafish_test_secret",
            1_767_139_200,
            br#"{"id":"evt_test","object":"event"}"#,
        );
        assert_eq!(
            sig,
            "t=1767139200,v1=85326cad9eb3d475e8a3d6615473baab7bbb9eb51598fefe3623bc3311e9a057"
        );
    }

    #[test]
    fn signature_covers_timestamp_and_body() {
        let body = br#"{"a":1}"#;
        let base = stripe_signature("whsec_x", 100, body);
        assert_ne!(base, stripe_signature("whsec_x", 101, body), "t is covered");
        assert_ne!(
            base,
            stripe_signature("whsec_x", 100, br#"{"a":2}"#),
            "body is covered"
        );
        assert_ne!(
            base,
            stripe_signature("whsec_y", 100, body),
            "secret is the key"
        );
    }
}
