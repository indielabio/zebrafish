//! Error type for `core`. Server crates map these onto the Stripe error
//! envelope (spec §5); `core` itself stays HTTP-agnostic.

use std::fmt;

/// Result alias used throughout `core`.
pub type Result<T> = std::result::Result<T, CoreError>;

/// Everything that can go wrong inside the domain layer.
#[derive(Debug)]
#[non_exhaustive]
pub enum CoreError {
    /// Underlying SQLite failure.
    Sqlite(rusqlite::Error),
    /// JSON (de)serialization failure (also covers `rng_state` encode/decode).
    Json(serde_json::Error),
    /// No object with the given id (maps to Stripe's 404 "No such ..." shape).
    NotFound {
        /// The resource type that was looked up, e.g. `"customer"`.
        kind: String,
        /// The id that did not resolve.
        id: String,
    },
    /// A precondition on the world state was violated.
    Conflict(String),
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "sqlite error: {e}"),
            Self::Json(e) => write!(f, "json error: {e}"),
            Self::NotFound { kind, id } => write!(f, "no such {kind}: '{id}'"),
            Self::Conflict(msg) => write!(f, "conflict: {msg}"),
        }
    }
}

impl std::error::Error for CoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(e) => Some(e),
            Self::Json(e) => Some(e),
            Self::NotFound { .. } | Self::Conflict(_) => None,
        }
    }
}

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
