//! The embedded DDL, applied at [`super::Store::open`].

/// Schema for all six tables (spec §4), idempotent via `IF NOT EXISTS`.
pub const SCHEMA: &str = include_str!("schema.sql");
