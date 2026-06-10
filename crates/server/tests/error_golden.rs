//! Golden tests for the Stripe error envelope (spec §5, §16.2). The
//! deterministic shapes mean snapshots are byte-stable with no redactions.

use insta::assert_json_snapshot;
use zebrafish_server::error::StripeError;

#[test]
fn invalid_request_envelope() {
    assert_json_snapshot!(StripeError::invalid_request("Missing required param: name.").to_json());
}

#[test]
fn not_found_envelope() {
    assert_json_snapshot!(StripeError::not_found("customer", "cus_missing").to_json());
}

#[test]
fn unauthorized_envelope() {
    assert_json_snapshot!(StripeError::unauthorized().to_json());
}

#[test]
fn api_error_envelope() {
    assert_json_snapshot!(StripeError::api_error("Internal error").to_json());
}

#[test]
fn idempotency_error_envelope() {
    assert_json_snapshot!(
        StripeError::idempotency(
            "Keys for idempotent requests can only be used with the same parameters."
        )
        .to_json()
    );
}

#[test]
fn unimplemented_envelope() {
    assert_json_snapshot!(StripeError::unimplemented("GET", "/v1/coupons").to_json());
}

#[test]
fn card_error_envelope() {
    let mut e = StripeError {
        status: axum::http::StatusCode::PAYMENT_REQUIRED,
        type_: "card_error",
        code: Some("card_declined".to_string()),
        decline_code: Some("insufficient_funds".to_string()),
        message: "Your card has insufficient funds.".to_string(),
        param: None,
        doc_url: Some("https://stripe.com/docs/error-codes/card-declined".to_string()),
    };
    e = e.with_code("card_declined");
    assert_json_snapshot!(e.to_json());
}
