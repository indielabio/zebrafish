//! Auth shim (spec §1 non-goals, §5).
//!
//! There is no auth realism: any non-empty `sk_test_*`/`pk_test_*` Bearer or
//! Basic credential is accepted. Only a *missing* credential is rejected, with
//! a 401 in Stripe's error shape.

use axum::extract::Request;
use axum::http::{HeaderMap, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;

use crate::error::StripeError;

/// Middleware: pass through when a credential is present, else 401.
pub async fn require_auth(req: Request, next: Next) -> Response {
    if has_credential(req.headers()) {
        next.run(req).await
    } else {
        StripeError::unauthorized().into_response()
    }
}

/// True when the request carries a non-empty Bearer or Basic credential.
fn has_credential(headers: &HeaderMap) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };

    if let Some(token) = value.strip_prefix("Bearer ") {
        return !token.trim().is_empty();
    }
    if let Some(b64) = value.strip_prefix("Basic ") {
        // Stripe sends the secret key as the Basic-auth username; password empty.
        return base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .is_some_and(|creds| !creds.split(':').next().unwrap_or("").is_empty());
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with(auth: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, HeaderValue::from_str(auth).unwrap());
        h
    }

    #[test]
    fn bearer_token_accepted() {
        assert!(has_credential(&headers_with("Bearer sk_test_abc")));
    }

    #[test]
    fn basic_credential_accepted() {
        let b64 = base64::engine::general_purpose::STANDARD.encode("sk_test_abc:");
        assert!(has_credential(&headers_with(&format!("Basic {b64}"))));
    }

    #[test]
    fn missing_and_empty_rejected() {
        assert!(!has_credential(&HeaderMap::new()));
        assert!(!has_credential(&headers_with("Bearer ")));
    }
}
