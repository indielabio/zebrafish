//! The Stripe `Event` model (spec §8, §11).
//!
//! Serialized form matches Stripe's webhook/event JSON exactly so apps can
//! deserialize it with their real SDKs.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The `data` envelope of an event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventData {
    /// A snapshot of the affected object, taken at emit time.
    pub object: Value,
    /// For `*.updated` events, the changed fields' prior values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_attributes: Option<Value>,
}

/// The `request` envelope: which API call triggered the event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRequest {
    /// The originating request id, if any.
    pub id: Option<String>,
    /// The originating `Idempotency-Key`, if any.
    pub idempotency_key: Option<String>,
}

/// A Stripe `Event`. `object` is always `"event"` and `livemode` always false.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StripeEvent {
    /// `evt_...`
    pub id: String,
    /// Always `"event"`.
    pub object: String,
    /// The pinned API version this event was rendered for.
    pub api_version: String,
    /// Virtual-clock creation time, unix seconds.
    pub created: i64,
    /// The affected object + optional previous attributes.
    pub data: EventData,
    /// Always false — zebrafish has no livemode.
    pub livemode: bool,
    /// Number of webhooks still pending delivery for this event.
    pub pending_webhooks: i64,
    /// The originating request.
    pub request: EventRequest,
    /// The event type, e.g. `"invoice.paid"`.
    #[serde(rename = "type")]
    pub type_: String,
}

/// Carried through a mutation so emitted events can reference the originating
/// request (spec §8 event `request` field).
#[derive(Debug, Default, Clone)]
pub struct RequestCtx {
    /// The originating request id (`req_...`).
    pub request_id: Option<String>,
    /// The originating `Idempotency-Key`.
    pub idempotency_key: Option<String>,
}
