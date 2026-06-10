//! Per-resource modules (spec §12).
//!
//! Each module is a unit struct implementing [`crate::resource::Resource`]:
//! metadata plus a `default_state` JSON builder whose shape is kept honest by
//! the contract tests against the vendored OpenAPI document
//! (`crates/core/openapi/spec3.sdk.json`). Lifecycle behaviour (cascades,
//! webhook delivery, the hosted checkout page) lands in WS-D/F/G.

pub mod charge;
pub mod checkout_session;
pub mod customer;
pub mod event;
pub mod invoice;
pub mod payment_intent;
pub mod payment_method;
pub mod price;
pub mod product;
pub mod subscription;
pub mod webhook_endpoint;
