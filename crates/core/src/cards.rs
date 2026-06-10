//! Magic test cards and the single card-outcome resolver (spec §9 Layer 1).
//!
//! `card_outcome(pan, context)` is the one function that decides what a card
//! does — used by the checkout page (WS-G), the renewal scheduler, and cascade
//! `when` clauses as `card.outcome`. The semantics mirror Stripe's published
//! test cards; docs link to Stripe's test-cards page rather than re-document.
//!
//! PANs are never persisted (spec §15): payment methods store only brand,
//! `last4`, expiry, and a fingerprint. Off-session outcome resolution therefore
//! goes through [`outcome_from_last4`] — the magic set's last-four digits are
//! pairwise distinct, so the stored `last4` identifies the behavior exactly.

use serde_json::{Value, json};

/// Whether the charge is customer-present (checkout) or merchant-initiated
/// (renewal / off-session).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargeContext {
    /// Customer-present: the fake checkout page, payment-method attach.
    Initial,
    /// Merchant-initiated: subscription renewals, off-session charges.
    OffSession,
}

/// What a charge attempt with this card does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardOutcome {
    /// The charge succeeds.
    Success,
    /// The charge is declined.
    Decline {
        /// Stripe error `code`, e.g. `"card_declined"`.
        code: &'static str,
        /// Stripe `decline_code`, e.g. `"insufficient_funds"`.
        decline_code: &'static str,
        /// Stripe's customer-facing wording.
        message: &'static str,
    },
}

impl CardOutcome {
    /// The value cascade `when` clauses match against as `card.outcome`.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Decline { .. } => "declined",
        }
    }

    /// The outcome as a cascade-context JSON fragment:
    /// `{ "outcome": "declined", "code": ..., "decline_code": ..., "message": ... }`.
    #[must_use]
    pub fn to_context(&self) -> Value {
        match self {
            Self::Success => json!({ "outcome": "success" }),
            Self::Decline {
                code,
                decline_code,
                message,
            } => json!({
                "outcome": "declined",
                "code": code,
                "decline_code": decline_code,
                "message": message,
            }),
        }
    }
}

/// Stripe's wording for a generic decline.
const DECLINED: CardOutcome = CardOutcome::Decline {
    code: "card_declined",
    decline_code: "generic_decline",
    message: "Your card was declined.",
};

/// Resolve a PAN to its charge outcome (spec §9 Layer 1, one function for all
/// callers). Unknown PANs succeed — only the magic set misbehaves.
#[must_use]
pub fn card_outcome(pan: &str, context: ChargeContext) -> CardOutcome {
    let digits: String = pan.chars().filter(char::is_ascii_digit).collect();
    match digits.as_str() {
        "4000000000000002" => DECLINED,
        "4000000000009995" => CardOutcome::Decline {
            code: "card_declined",
            decline_code: "insufficient_funds",
            message: "Your card has insufficient funds.",
        },
        // Attaches fine, then fails on renewal / off-session charges.
        "4000000000000341" => match context {
            ChargeContext::Initial => CardOutcome::Success,
            ChargeContext::OffSession => DECLINED,
        },
        "4000000000000069" => CardOutcome::Decline {
            code: "expired_card",
            decline_code: "expired_card",
            message: "Your card has expired.",
        },
        "4000000000000127" => CardOutcome::Decline {
            code: "incorrect_cvc",
            decline_code: "incorrect_cvc",
            message: "Your card's security code is incorrect.",
        },
        _ => CardOutcome::Success,
    }
}

/// Resolve a *stored* payment method's `last4` to a charge outcome. PANs are
/// never persisted (spec §15), so renewals re-derive behavior from `last4`;
/// the magic set's last-fours are pairwise distinct so this is lossless.
#[must_use]
pub fn outcome_from_last4(last4: &str, context: ChargeContext) -> CardOutcome {
    let pan = match last4 {
        "0002" => "4000000000000002",
        "9995" => "4000000000009995",
        "0341" => "4000000000000341",
        "0069" => "4000000000000069",
        "0127" => "4000000000000127",
        _ => return CardOutcome::Success,
    };
    card_outcome(pan, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ChargeContext::{Initial, OffSession};

    /// Every magic card × context, per the spec §9 table.
    #[test]
    fn magic_card_table() {
        let cases: &[(&str, ChargeContext, &str, Option<&str>)] = &[
            ("4242424242424242", Initial, "success", None),
            ("4242424242424242", OffSession, "success", None),
            ("5555555555554444", Initial, "success", None),
            ("378282246310005", Initial, "success", None),
            (
                "4000000000000002",
                Initial,
                "declined",
                Some("generic_decline"),
            ),
            (
                "4000000000000002",
                OffSession,
                "declined",
                Some("generic_decline"),
            ),
            (
                "4000000000009995",
                Initial,
                "declined",
                Some("insufficient_funds"),
            ),
            (
                "4000000000009995",
                OffSession,
                "declined",
                Some("insufficient_funds"),
            ),
            ("4000000000000341", Initial, "success", None),
            (
                "4000000000000341",
                OffSession,
                "declined",
                Some("generic_decline"),
            ),
            (
                "4000000000000069",
                Initial,
                "declined",
                Some("expired_card"),
            ),
            (
                "4000000000000127",
                Initial,
                "declined",
                Some("incorrect_cvc"),
            ),
            // Unknown cards succeed in both contexts.
            ("4111111111111111", Initial, "success", None),
            ("4111111111111111", OffSession, "success", None),
        ];
        for (pan, ctx, label, decline) in cases {
            let outcome = card_outcome(pan, *ctx);
            assert_eq!(outcome.label(), *label, "pan {pan} ctx {ctx:?}");
            if let Some(expected) = decline {
                let CardOutcome::Decline { decline_code, .. } = outcome else {
                    panic!("pan {pan} ctx {ctx:?}: expected a decline");
                };
                assert_eq!(decline_code, *expected, "pan {pan} ctx {ctx:?}");
            }
        }
    }

    #[test]
    fn pan_accepts_separators() {
        assert_eq!(
            card_outcome("4000 0000 0000 0002", Initial).label(),
            "declined"
        );
        assert_eq!(
            card_outcome("4242-4242-4242-4242", Initial).label(),
            "success"
        );
    }

    #[test]
    fn last4_resolution_matches_pan_resolution() {
        for (pan, last4) in [
            ("4000000000000002", "0002"),
            ("4000000000009995", "9995"),
            ("4000000000000341", "0341"),
            ("4000000000000069", "0069"),
            ("4000000000000127", "0127"),
            ("4242424242424242", "4242"),
        ] {
            for ctx in [Initial, OffSession] {
                assert_eq!(
                    outcome_from_last4(last4, ctx),
                    card_outcome(pan, ctx),
                    "last4 {last4} ctx {ctx:?}"
                );
            }
        }
    }

    #[test]
    fn context_fragment_carries_decline_details() {
        let ctx = card_outcome("4000000000009995", Initial).to_context();
        assert_eq!(ctx["outcome"], "declined");
        assert_eq!(ctx["decline_code"], "insufficient_funds");
        assert_eq!(
            card_outcome("4242424242424242", Initial).to_context()["outcome"],
            "success"
        );
    }
}
