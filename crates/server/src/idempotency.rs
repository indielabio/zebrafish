//! Idempotency-Key replay cache (spec §5).
//!
//! Keyed by `(Idempotency-Key, method+path, body-hash)`. A repeat of an
//! identical request replays the stored response; the same key with a different
//! body is an `idempotency_error`. Entries expire after 24 virtual hours.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

/// 24 hours in seconds (virtual time).
const TTL_SECONDS: i64 = 24 * 60 * 60;

/// SHA-256 hex digest of a request body.
#[must_use]
pub fn body_hash(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[derive(Debug, Clone)]
struct Entry {
    body_hash: String,
    status: u16,
    response_body: Vec<u8>,
    stored_at: i64,
}

/// The outcome of consulting the cache for a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// Not seen before — proceed and then [`IdempotencyStore::record`] the result.
    Miss,
    /// Seen with the same body — replay this stored response.
    Replay {
        /// Stored HTTP status.
        status: u16,
        /// Stored response body.
        body: Vec<u8>,
    },
    /// Seen with the same key but a different body — return an idempotency error.
    Conflict,
}

/// In-memory store of recent idempotent responses.
#[derive(Debug, Default)]
pub struct IdempotencyStore {
    entries: HashMap<String, Entry>,
}

fn composite(key: &str, method_path: &str) -> String {
    format!("{key}\u{0}{method_path}")
}

impl IdempotencyStore {
    /// Look up `key` for the given `method_path` and request `body` at virtual
    /// time `now`, evicting any expired entry first.
    pub fn check(&mut self, key: &str, method_path: &str, body: &[u8], now: i64) -> Lookup {
        let composite = composite(key, method_path);
        if let Some(entry) = self.entries.get(&composite) {
            if now - entry.stored_at >= TTL_SECONDS {
                self.entries.remove(&composite);
                return Lookup::Miss;
            }
            if entry.body_hash == body_hash(body) {
                return Lookup::Replay {
                    status: entry.status,
                    body: entry.response_body.clone(),
                };
            }
            return Lookup::Conflict;
        }
        Lookup::Miss
    }

    /// Record the response for a key so a future identical request can replay it.
    pub fn record(
        &mut self,
        key: &str,
        method_path: &str,
        body: &[u8],
        status: u16,
        response_body: Vec<u8>,
        now: i64,
    ) {
        self.entries.insert(
            composite(key, method_path),
            Entry {
                body_hash: body_hash(body),
                status,
                response_body,
                stored_at: now,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn miss_then_replay() {
        let mut s = IdempotencyStore::default();
        assert_eq!(s.check("k1", "POST /v1/customers", b"a=1", 0), Lookup::Miss);
        s.record("k1", "POST /v1/customers", b"a=1", 200, b"{}".to_vec(), 0);
        assert_eq!(
            s.check("k1", "POST /v1/customers", b"a=1", 10),
            Lookup::Replay {
                status: 200,
                body: b"{}".to_vec()
            },
        );
    }

    #[test]
    fn same_key_different_body_conflicts() {
        let mut s = IdempotencyStore::default();
        s.record("k1", "POST /v1/customers", b"a=1", 200, b"{}".to_vec(), 0);
        assert_eq!(
            s.check("k1", "POST /v1/customers", b"a=2", 5),
            Lookup::Conflict,
        );
    }

    #[test]
    fn expires_after_ttl() {
        let mut s = IdempotencyStore::default();
        s.record("k1", "POST /v1/customers", b"a=1", 200, b"{}".to_vec(), 0);
        assert_eq!(
            s.check("k1", "POST /v1/customers", b"a=1", TTL_SECONDS),
            Lookup::Miss,
        );
    }
}
