//! In-process notification bus (spec §6.1, §11).
//!
//! Built on a `tokio::sync::broadcast` channel — the `sync` feature only, so
//! `core` stays free of any HTTP/runtime dependency. The async server and
//! dashboard `subscribe()` and forward each [`Notification`] as an SSE frame.

use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

/// Capacity of the broadcast channel. A slow subscriber that overflows this
/// receives `Lagged` and re-syncs from the store — mutations never block on it.
const CAPACITY: usize = 1024;

/// A change worth telling subscribers about. Serializes to the dashboard's
/// `{ kind, payload }` SSE shape (spec §11).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum Notification {
    /// An object was created or updated.
    ObjectWritten(Value),
    /// An event was emitted.
    EventEmitted(Value),
    /// A webhook delivery attempt completed.
    DeliveryAttempted(Value),
    /// The virtual clock moved.
    ClockAdvanced(Value),
    /// A chaos rule was added, removed, or consumed.
    ChaosChanged(Value),
}

/// Fan-out sender for [`Notification`]s.
#[derive(Debug, Clone)]
pub struct NotificationBus {
    tx: broadcast::Sender<Notification>,
}

impl NotificationBus {
    /// Create a bus with the default capacity.
    #[must_use]
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(CAPACITY);
        Self { tx }
    }

    /// Subscribe to the stream of notifications.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.tx.subscribe()
    }

    /// Publish a notification. Drops silently when there are no subscribers.
    pub fn publish(&self, n: Notification) {
        let _ = self.tx.send(n);
    }
}

impl Default for NotificationBus {
    fn default() -> Self {
        Self::new()
    }
}
