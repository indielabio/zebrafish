//! The Stripe error envelope and status mapping (spec §5).
//!
//! Every error response is `{ "error": { type, code, decline_code?, message,
//! param, doc_url } }` with the status mapping:
//! `card_error` 402 · `invalid_request_error` 400 (404 for unknown id) ·
//! `api_error` 500 · auth 401 · `rate_limit_error` 429.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use zebrafish_core::CoreError;

/// A Stripe-shaped error.
#[derive(Debug, Clone)]
pub struct StripeError {
    /// HTTP status to return.
    pub status: StatusCode,
    /// Error category, e.g. `"invalid_request_error"`.
    pub type_: &'static str,
    /// Machine-readable code, e.g. `"card_declined"`.
    pub code: Option<String>,
    /// Card-specific decline reason.
    pub decline_code: Option<String>,
    /// Human-readable message.
    pub message: String,
    /// The offending request parameter, if any.
    pub param: Option<String>,
    /// Documentation URL for the error code.
    pub doc_url: Option<String>,
}

impl StripeError {
    fn new(status: StatusCode, type_: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            type_,
            code: None,
            decline_code: None,
            message: message.into(),
            param: None,
            doc_url: None,
        }
    }

    /// 400 invalid request.
    #[must_use]
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request_error", message)
    }

    /// 404 with Stripe's `No such {kind}: '{id}'` wording.
    #[must_use]
    pub fn not_found(kind: &str, id: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "invalid_request_error",
            format!("No such {kind}: '{id}'"),
        )
    }

    /// 401 missing/invalid credentials.
    #[must_use]
    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "invalid_request_error",
            "You did not provide an API key. You need to provide your API key in \
             the Authorization header, using Bearer auth (e.g. 'Authorization: \
             Bearer sk_test_...').",
        )
    }

    /// 500 internal error.
    #[must_use]
    pub fn api_error(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "api_error", message)
    }

    /// 400 idempotency conflict (same key, different body).
    #[must_use]
    pub fn idempotency(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "idempotency_error", message)
    }

    /// 501 for endpoints zebrafish does not implement yet (spec §1).
    #[must_use]
    pub fn unimplemented(method: &str, path: &str) -> Self {
        let mut e = Self::new(
            StatusCode::NOT_IMPLEMENTED,
            "invalid_request_error",
            format!(
                "zebrafish does not implement {method} {path} yet. \
                 Coverage: /_dashboard/coverage. Contribute: \
                 https://github.com/indielabio/zebrafish/blob/main/docs/CONTRIBUTING.md"
            ),
        );
        e.code = Some("not_implemented".to_string());
        e
    }

    /// Attach a `code`.
    #[must_use]
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// The error body as JSON (the value of the top-level `error` key).
    fn body(&self) -> Value {
        let mut obj = json!({
            "type": self.type_,
            "message": self.message,
            "code": self.code,
            "param": self.param,
        });
        if let Some(dc) = &self.decline_code {
            obj["decline_code"] = json!(dc);
        }
        if let Some(url) = &self.doc_url {
            obj["doc_url"] = json!(url);
        }
        obj
    }

    /// The full error envelope `{ "error": { ... } }`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({ "error": self.body() })
    }
}

impl std::fmt::Display for StripeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.type_, self.message)
    }
}

impl std::error::Error for StripeError {}

impl IntoResponse for StripeError {
    fn into_response(self) -> Response {
        (self.status, Json(self.to_json())).into_response()
    }
}

impl From<CoreError> for StripeError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::NotFound { kind, id } => Self::not_found(&kind, &id),
            CoreError::Conflict(msg) => Self::invalid_request(msg),
            other => Self::api_error(other.to_string()),
        }
    }
}

/// Convenience alias for handler results.
pub type ApiResult<T> = std::result::Result<T, StripeError>;
