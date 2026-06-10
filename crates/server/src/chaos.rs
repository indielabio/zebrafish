//! The chaos engine (spec §9): rule matching for the API plane, and the
//! per-request `Zebrafish-Fail` header.
//!
//! Layer 2 rules are stored in the `chaos_rules` table (core owns persistence,
//! `times` decrement, and TTL); this module interprets them for `/v1`
//! requests. Webhook-side kinds (`webhook_*`) are interpreted by the delivery
//! worker. Layer 3 — the `Zebrafish-Fail` header — is resolved entirely from
//! the request itself, so parallel tests can inject failures with no shared
//! state.

use axum::extract::{OriginalUri, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use std::time::Duration;

use crate::error::StripeError;
use crate::state::AppState;

/// The chaos action kinds spec §9 defines.
pub const ACTION_KINDS: &[&str] = &[
    "error",
    "delay",
    "timeout",
    "webhook_drop",
    "webhook_duplicate",
    "webhook_delay",
    "webhook_reorder",
];

/// Minimal glob: `*` matches any (possibly empty) substring. Linear-time
/// two-pointer match — patterns are tiny (`/v1/subscriptions*`, `invoice.*`).
#[must_use]
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Middleware on the `/v1` plane: apply the `Zebrafish-Fail` header, then the
/// first matching stored API rule (`error` / `delay` / `timeout`).
pub async fn apply_chaos(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    req: Request,
    next: Next,
) -> Response {
    // Layer 3 — per-request header, no global state (spec §9).
    if let Some(value) = req
        .headers()
        .get("zebrafish-fail")
        .and_then(|v| v.to_str().ok())
    {
        match parse_fail_header(value) {
            Some(FailDirective::Error(err)) => return err.into_response(),
            Some(FailDirective::Delay(ms)) => {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            Some(FailDirective::Timeout) => {
                return std::future::pending().await;
            }
            None => {
                return StripeError::invalid_request(format!(
                    "Unrecognized Zebrafish-Fail value: '{value}'. Expected one of: \
                     card_declined, api_error, rate_limit, delay=<ms>, timeout."
                ))
                .into_response();
            }
        }
    }

    // Layer 2 — stored rules. One world lock to find + consume.
    let action = {
        let mut world = state.world();
        let rules = world.list_chaos_rules().unwrap_or_default();
        let matched = rules
            .into_iter()
            .find(|rule| rule_matches_request(&rule.rule, req.method().as_str(), uri.path()));
        match matched {
            Some(rule) => {
                let _ = world.consume_chaos_rule(&rule.id);
                Some(rule.rule["action"].clone())
            }
            None => None,
        }
    };

    if let Some(action) = action {
        match action.get("kind").and_then(Value::as_str) {
            Some("error") => return rule_error(&action).into_response(),
            Some("delay") => {
                let ms = action
                    .pointer("/ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(1000);
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            Some("timeout") => {
                // Hold the connection open until the client gives up (spec §9).
                return std::future::pending().await;
            }
            _ => {}
        }
    }

    next.run(req).await
}

/// Whether an API-plane rule (`error`/`delay`/`timeout`) matches a request.
/// Rules carrying `match.event_type` or a `webhook_*` action belong to the
/// delivery worker and never match here.
fn rule_matches_request(rule: &Value, method: &str, path: &str) -> bool {
    let kind = rule
        .pointer("/action/kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if kind.starts_with("webhook_") {
        return false;
    }
    let m = &rule["match"];
    if m.get("event_type").and_then(Value::as_str).is_some() {
        return false;
    }
    if let Some(want) = m.get("method").and_then(Value::as_str)
        && !want.eq_ignore_ascii_case(method)
    {
        return false;
    }
    if let Some(glob) = m.get("path_glob").and_then(Value::as_str)
        && !glob_match(glob, path)
    {
        return false;
    }
    if let Some(id) = m.get("object_id").and_then(Value::as_str)
        && !path.split('/').any(|seg| seg == id)
    {
        return false;
    }
    true
}

/// Build the Stripe-shaped error a rule's `action.error` describes.
fn rule_error(action: &Value) -> StripeError {
    let spec = &action["error"];
    let type_ = match spec.get("type").and_then(Value::as_str) {
        Some("card_error") => "card_error",
        Some("invalid_request_error") => "invalid_request_error",
        Some("rate_limit_error" | "rate_limit") => "rate_limit_error",
        Some("idempotency_error") => "idempotency_error",
        _ => "api_error",
    };
    let default_status = match type_ {
        "card_error" => 402,
        "invalid_request_error" | "idempotency_error" => 400,
        "rate_limit_error" => 429,
        _ => 500,
    };
    let status = spec
        .get("http_status")
        .and_then(Value::as_u64)
        .and_then(|s| u16::try_from(s).ok())
        .and_then(|s| StatusCode::from_u16(s).ok())
        .unwrap_or_else(|| {
            StatusCode::from_u16(default_status).expect("static statuses are valid")
        });
    let message = spec
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Injected by zebrafish chaos rule.")
        .to_string();

    let mut err = StripeError::chaos(status, type_, message);
    err.code = spec.get("code").and_then(Value::as_str).map(str::to_string);
    err.decline_code = spec
        .get("decline_code")
        .and_then(Value::as_str)
        .map(str::to_string);
    err
}

/// A parsed `Zebrafish-Fail` header.
enum FailDirective {
    Error(StripeError),
    Delay(u64),
    Timeout,
}

/// Parse the header (spec §9 Layer 3): `card_declined`, `api_error`,
/// `rate_limit`, `delay=<ms>`, `timeout`.
fn parse_fail_header(value: &str) -> Option<FailDirective> {
    let value = value.trim();
    if let Some(ms) = value.strip_prefix("delay=") {
        let ms = ms.trim_end_matches("ms").parse().ok()?;
        return Some(FailDirective::Delay(ms));
    }
    match value {
        "card_declined" => Some(FailDirective::Error(StripeError::card_declined())),
        "api_error" => Some(FailDirective::Error(StripeError::api_error(
            "Injected by Zebrafish-Fail header.",
        ))),
        "rate_limit" => Some(FailDirective::Error(StripeError::rate_limited())),
        "timeout" => Some(FailDirective::Timeout),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn glob_matches() {
        assert!(glob_match("/v1/subscriptions*", "/v1/subscriptions"));
        assert!(glob_match("/v1/subscriptions*", "/v1/subscriptions/sub_1"));
        assert!(!glob_match("/v1/subscriptions*", "/v1/customers"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("invoice.*", "invoice.paid"));
        assert!(glob_match("*paid*", "invoice.paid"));
        assert!(!glob_match("invoice.*", "invoices"));
        assert!(glob_match("a*b*c", "a-x-b-y-c"));
        assert!(!glob_match("a*b*c", "a-x-b-y"));
    }

    #[test]
    fn api_rule_matching_honours_method_path_and_object_id() {
        let rule = json!({
            "match": { "method": "POST", "path_glob": "/v1/subscriptions*", "object_id": null },
            "action": { "kind": "error" },
        });
        assert!(rule_matches_request(&rule, "POST", "/v1/subscriptions"));
        assert!(!rule_matches_request(&rule, "GET", "/v1/subscriptions"));
        assert!(!rule_matches_request(&rule, "POST", "/v1/customers"));

        let by_id = json!({
            "match": { "object_id": "cus_42" },
            "action": { "kind": "delay" },
        });
        assert!(rule_matches_request(&by_id, "GET", "/v1/customers/cus_42"));
        assert!(!rule_matches_request(&by_id, "GET", "/v1/customers/cus_43"));
    }

    #[test]
    fn webhook_rules_never_match_api_requests() {
        let rule = json!({
            "match": { "event_type": "invoice.*" },
            "action": { "kind": "webhook_drop" },
        });
        assert!(!rule_matches_request(&rule, "POST", "/v1/invoices"));
        let timeout_on_events = json!({
            "match": { "event_type": "*" },
            "action": { "kind": "timeout" },
        });
        assert!(!rule_matches_request(
            &timeout_on_events,
            "POST",
            "/v1/invoices"
        ));
    }

    #[test]
    fn rule_error_maps_types_to_statuses() {
        let err = rule_error(&json!({
            "error": { "type": "card_error", "code": "card_declined",
                       "decline_code": "insufficient_funds", "message": "nope" }
        }));
        assert_eq!(err.status.as_u16(), 402);
        assert_eq!(err.code.as_deref(), Some("card_declined"));
        assert_eq!(err.decline_code.as_deref(), Some("insufficient_funds"));

        let err = rule_error(&json!({
            "error": { "type": "api_error", "http_status": 503 }
        }));
        assert_eq!(err.status.as_u16(), 503);
    }

    #[test]
    fn fail_header_parses() {
        assert!(matches!(
            parse_fail_header("delay=250"),
            Some(FailDirective::Delay(250))
        ));
        assert!(matches!(
            parse_fail_header("card_declined"),
            Some(FailDirective::Error(_))
        ));
        assert!(parse_fail_header("explode").is_none());
    }
}
