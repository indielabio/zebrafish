//! SQLite blob store (spec §4).
//!
//! One `Mutex<Connection>` — there is a single local user, so no async pool.
//! Stripe objects are stored as their full `api_state` JSON, never relationally
//! modeled. Mutations run through [`Store::transaction`] so the world row (clock
//! + RNG state) is committed atomically with the data it produced.

mod schema;

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;

use crate::error::Result;

/// A row of the `objects` table.
#[derive(Debug, Clone)]
pub struct StoredObject {
    /// The object id (also the JSON `id`).
    pub id: String,
    /// The Stripe object type (the JSON `object`).
    pub type_: String,
    /// The full `api_state` JSON.
    pub api_state: Value,
    /// Virtual-clock creation time, unix seconds.
    pub created: i64,
    /// Soft-deletion flag.
    pub deleted: bool,
}

/// The single `world` row: virtual clock, seed, and serialized RNG state.
#[derive(Debug, Clone)]
pub struct WorldRow {
    /// Virtual clock, unix seconds.
    pub now_unix: i64,
    /// The world seed.
    pub seed: u64,
    /// Serialized ChaCha RNG state (see [`crate::rng::WorldRng`]).
    pub rng_state: Vec<u8>,
    /// The pinned Stripe API version.
    pub stripe_api_version: String,
}

/// Owns the SQLite connection behind a mutex.
#[derive(Debug)]
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Open a store at `path`, or in memory when `path` is `":memory:"`.
    /// Applies WAL + pragmas and runs migrations.
    pub fn open(path: &str) -> Result<Self> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory()?
        } else {
            Connection::open(Path::new(path))?
        };

        // WAL returns a row ("wal" on disk, "memory" in memory) — read it back.
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL;", [], |r| r.get(0))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(schema::SCHEMA)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Run `f` inside a single transaction, committing on `Ok`.
    pub fn transaction<R>(&self, f: impl FnOnce(&Transaction) -> Result<R>) -> Result<R> {
        let mut guard = self.conn.lock().expect("store mutex poisoned");
        let tx = guard.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Run `f` against the connection for read-only access.
    pub fn read<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let guard = self.conn.lock().expect("store mutex poisoned");
        f(&guard)
    }
}

// --- object helpers (take `&Connection`; `&Transaction` deref-coerces) -------

/// Insert or replace an object row.
pub fn put_object(conn: &Connection, obj: &StoredObject) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO objects (id, type, api_state, created, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            obj.id,
            obj.type_,
            serde_json::to_string(&obj.api_state)?,
            obj.created,
            i64::from(obj.deleted),
        ],
    )?;
    Ok(())
}

/// Fetch a full object row by id.
pub fn get(conn: &Connection, id: &str) -> Result<Option<StoredObject>> {
    let row = conn
        .query_row(
            "SELECT id, type, api_state, created, deleted FROM objects WHERE id = ?1",
            params![id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?;

    match row {
        None => Ok(None),
        Some((id, type_, api_state, created, deleted)) => Ok(Some(StoredObject {
            id,
            type_,
            api_state: serde_json::from_str(&api_state)?,
            created,
            deleted: deleted != 0,
        })),
    }
}

/// Convenience: fetch just an object's `api_state` JSON.
pub fn get_object(conn: &Connection, id: &str) -> Result<Option<Value>> {
    Ok(get(conn, id)?.map(|o| o.api_state))
}

/// All non-deleted objects of a type, newest first (created desc, id desc).
pub fn query_by_type(conn: &Connection, type_: &str) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT api_state FROM objects
         WHERE type = ?1 AND deleted = 0
         ORDER BY created DESC, id DESC",
    )?;
    let rows = stmt.query_map(params![type_], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for raw in rows {
        out.push(serde_json::from_str(&raw?)?);
    }
    Ok(out)
}

/// All non-deleted objects whose `$.customer` equals `customer_id` (uses the
/// `idx_objects_customer` expression index).
pub fn query_by_customer(conn: &Connection, customer_id: &str) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT api_state FROM objects
         WHERE json_extract(api_state, '$.customer') = ?1 AND deleted = 0
         ORDER BY created DESC, id DESC",
    )?;
    let rows = stmt.query_map(params![customer_id], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for raw in rows {
        out.push(serde_json::from_str(&raw?)?);
    }
    Ok(out)
}

/// Every object as `(id, api_state)`, ordered by id — for deterministic dumps.
pub fn all_objects(conn: &Connection) -> Result<Vec<(String, Value)>> {
    let mut stmt = conn.prepare("SELECT id, api_state FROM objects ORDER BY id ASC")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, raw) = row?;
        out.push((id, serde_json::from_str(&raw)?));
    }
    Ok(out)
}

// --- event helpers -----------------------------------------------------------

/// Insert an event row.
pub fn put_event(conn: &Connection, id: &str, type_: &str, payload: &Value, created: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO events (id, type, payload, created) VALUES (?1, ?2, ?3, ?4)",
        params![id, type_, serde_json::to_string(payload)?, created],
    )?;
    Ok(())
}

/// Fetch an event payload by id.
pub fn get_event(conn: &Connection, id: &str) -> Result<Option<Value>> {
    let raw = conn
        .query_row(
            "SELECT payload FROM events WHERE id = ?1",
            params![id],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    match raw {
        None => Ok(None),
        Some(s) => Ok(Some(serde_json::from_str(&s)?)),
    }
}

// --- world row helpers -------------------------------------------------------

/// Insert or replace the singleton world row.
pub fn save_world_row(conn: &Connection, row: &WorldRow) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO world (id, now_unix, seed, rng_state, stripe_api_version)
         VALUES (1, ?1, ?2, ?3, ?4)",
        params![
            row.now_unix,
            row.seed as i64,
            row.rng_state,
            row.stripe_api_version,
        ],
    )?;
    Ok(())
}

/// Load the singleton world row, if the world has been booted before.
pub fn load_world_row(conn: &Connection) -> Result<Option<WorldRow>> {
    let row = conn
        .query_row(
            "SELECT now_unix, seed, rng_state, stripe_api_version FROM world WHERE id = 1",
            [],
            |r| {
                Ok(WorldRow {
                    now_unix: r.get(0)?,
                    seed: r.get::<_, i64>(1)? as u64,
                    rng_state: r.get(2)?,
                    stripe_api_version: r.get(3)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Delete all object/event/delivery/chaos state, keeping the world row
/// (seed + clock + RNG) and any registered webhook endpoints (spec §9 reset).
pub fn flush_data(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM objects;
         DELETE FROM events;
         DELETE FROM deliveries;
         DELETE FROM chaos_rules;",
    )?;
    Ok(())
}
