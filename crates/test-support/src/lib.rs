//! Shared test harness for zebrafish (dev-dependency only).
//!
//! Will provide `CaptureServer`, `Zebrafish::spawn`, and `WorldBuilder`.
//! This is a skeleton; real implementation lands with WS-A/WS-B.

/// Re-exported so harness consumers build worlds against the same core types
/// the server uses (the `WorldBuilder` lands in WS-A).
pub use zebrafish_core;

/// Default port the emulator listens on (spec §5).
pub const DEFAULT_PORT: u16 = 4242;
