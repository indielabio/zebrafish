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

/// A row of the `webhook_endpoints` table (spec §8). Survives `flush_data`.
#[derive(Debug, Clone)]
pub struct WebhookEndpointRow {
    /// The endpoint id (`we_...`).
    pub id: String,
    /// Destination URL.
    pub url: String,
    /// Signing secret (`whsec_...`).
    pub secret: String,
    /// Event-type filters (`["*"]` matches everything).
    pub events: Vec<String>,
    /// Virtual-clock creation time, unix seconds.
    pub created: i64,
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
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (id, raw) = row?;
        out.push((id, serde_json::from_str(&raw)?));
    }
    Ok(out)
}

// --- event helpers -----------------------------------------------------------

/// Insert an event row.
pub fn put_event(
    conn: &Connection,
    id: &str,
    type_: &str,
    payload: &Value,
    created: i64,
) -> Result<()> {
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

/// All event payloads, newest first (created desc, id desc).
pub fn list_events(conn: &Connection) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare("SELECT payload FROM events ORDER BY created DESC, id DESC")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for raw in rows {
        out.push(serde_json::from_str(&raw?)?);
    }
    Ok(out)
}

// --- webhook endpoint helpers --------------------------------------------------

/// Insert or replace a webhook endpoint row.
pub fn put_webhook_endpoint(conn: &Connection, row: &WebhookEndpointRow) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO webhook_endpoints (id, url, secret, events, created)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            row.id,
            row.url,
            row.secret,
            serde_json::to_string(&row.events)?,
            row.created,
        ],
    )?;
    Ok(())
}

fn webhook_endpoint_from_row(
    r: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, String, String, i64)> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
}

/// Fetch a webhook endpoint row by id.
pub fn get_webhook_endpoint(conn: &Connection, id: &str) -> Result<Option<WebhookEndpointRow>> {
    let row = conn
        .query_row(
            "SELECT id, url, secret, events, created FROM webhook_endpoints WHERE id = ?1",
            params![id],
            webhook_endpoint_from_row,
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some((id, url, secret, events, created)) => Ok(Some(WebhookEndpointRow {
            id,
            url,
            secret,
            events: serde_json::from_str(&events)?,
            created,
        })),
    }
}

/// All webhook endpoint rows, newest first (created desc, id desc).
pub fn list_webhook_endpoints(conn: &Connection) -> Result<Vec<WebhookEndpointRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, url, secret, events, created FROM webhook_endpoints
         ORDER BY created DESC, id DESC",
    )?;
    let rows = stmt.query_map([], webhook_endpoint_from_row)?;
    let mut out = Vec::new();
    for row in rows {
        let (id, url, secret, events, created) = row?;
        out.push(WebhookEndpointRow {
            id,
            url,
            secret,
            events: serde_json::from_str(&events)?,
            created,
        });
    }
    Ok(out)
}

/// Hard-delete a webhook endpoint row. Returns whether a row was removed.
pub fn delete_webhook_endpoint(conn: &Connection, id: &str) -> Result<bool> {
    let n = conn.execute("DELETE FROM webhook_endpoints WHERE id = ?1", params![id])?;
    Ok(n > 0)
}

// --- delivery helpers ----------------------------------------------------------

/// A row of the `deliveries` table (spec §4, §8): one webhook delivery attempt.
#[derive(Debug, Clone)]
pub struct DeliveryRow {
    /// The delivery id (`del_...`).
    pub id: String,
    /// The delivered event.
    pub event_id: String,
    /// The destination endpoint.
    pub endpoint_id: String,
    /// 1-based attempt number per (event, endpoint).
    pub attempt: i64,
    /// The exact JSON body sent (the bytes the signature covers).
    pub request_body: String,
    /// The `Stripe-Signature` header value sent.
    pub signature: String,
    /// HTTP status returned by the app; `None` = connection failure/timeout.
    pub status_code: Option<i64>,
    /// The app's response body, if any.
    pub response_body: Option<String>,
    /// Wall-clock duration of the attempt.
    pub duration_ms: Option<i64>,
    /// Virtual-clock time of the attempt.
    pub delivered_at: i64,
}

impl DeliveryRow {
    /// The row as dashboard/config-plane JSON.
    #[must_use]
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "object": "delivery",
            "event_id": self.event_id,
            "endpoint_id": self.endpoint_id,
            "attempt": self.attempt,
            "request_body": self.request_body,
            "signature": self.signature,
            "status_code": self.status_code,
            "response_body": self.response_body,
            "duration_ms": self.duration_ms,
            "delivered_at": self.delivered_at,
        })
    }
}

/// Insert a delivery-attempt row.
pub fn put_delivery(conn: &Connection, row: &DeliveryRow) -> Result<()> {
    conn.execute(
        "INSERT INTO deliveries (id, event_id, endpoint_id, attempt, request_body,
                                 signature, status_code, response_body, duration_ms, delivered_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            row.id,
            row.event_id,
            row.endpoint_id,
            row.attempt,
            row.request_body,
            row.signature,
            row.status_code,
            row.response_body,
            row.duration_ms,
            row.delivered_at,
        ],
    )?;
    Ok(())
}

fn delivery_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DeliveryRow> {
    Ok(DeliveryRow {
        id: r.get(0)?,
        event_id: r.get(1)?,
        endpoint_id: r.get(2)?,
        attempt: r.get(3)?,
        request_body: r.get(4)?,
        signature: r.get(5)?,
        status_code: r.get(6)?,
        response_body: r.get(7)?,
        duration_ms: r.get(8)?,
        delivered_at: r.get(9)?,
    })
}

const DELIVERY_COLS: &str = "id, event_id, endpoint_id, attempt, request_body, \
                             signature, status_code, response_body, duration_ms, delivered_at";

/// All delivery attempts, newest first.
pub fn list_deliveries(conn: &Connection) -> Result<Vec<DeliveryRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {DELIVERY_COLS} FROM deliveries ORDER BY delivered_at DESC, id DESC"
    ))?;
    let rows = stmt.query_map([], delivery_from_row)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Delivery attempts for one event, oldest first (attempt order).
pub fn deliveries_for_event(conn: &Connection, event_id: &str) -> Result<Vec<DeliveryRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {DELIVERY_COLS} FROM deliveries WHERE event_id = ?1
         ORDER BY delivered_at ASC, attempt ASC, id ASC"
    ))?;
    let rows = stmt.query_map(params![event_id], delivery_from_row)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// The next attempt number for (event, endpoint): `max(attempt) + 1`.
pub fn next_attempt(conn: &Connection, event_id: &str, endpoint_id: &str) -> Result<i64> {
    let max: Option<i64> = conn.query_row(
        "SELECT MAX(attempt) FROM deliveries WHERE event_id = ?1 AND endpoint_id = ?2",
        params![event_id, endpoint_id],
        |r| r.get(0),
    )?;
    Ok(max.unwrap_or(0) + 1)
}

// --- chaos rule helpers ----------------------------------------------------------

/// A row of the `chaos_rules` table (spec §4, §9).
#[derive(Debug, Clone)]
pub struct ChaosRuleRow {
    /// The rule id (`chaos_...`).
    pub id: String,
    /// The rule JSON (`match` + `action`) exactly as posted.
    pub rule: Value,
    /// Remaining applications; `None` = unlimited.
    pub remaining: Option<i64>,
    /// Virtual-clock expiry; `None` = no TTL.
    pub expires_at: Option<i64>,
}

impl ChaosRuleRow {
    /// The row as config-plane JSON.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut v = self.rule.clone();
        v["id"] = Value::String(self.id.clone());
        v["remaining"] = self.remaining.map_or(Value::Null, Into::into);
        v["expires_at"] = self.expires_at.map_or(Value::Null, Into::into);
        v
    }
}

/// Insert or replace a chaos rule.
pub fn put_chaos_rule(conn: &Connection, row: &ChaosRuleRow) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO chaos_rules (id, rule, remaining, expires_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            row.id,
            serde_json::to_string(&row.rule)?,
            row.remaining,
            row.expires_at,
        ],
    )?;
    Ok(())
}

/// All chaos rules that are neither exhausted nor expired at virtual time
/// `now`, oldest id first (creation order — ids are drawn sequentially).
pub fn list_chaos_rules(conn: &Connection, now: i64) -> Result<Vec<ChaosRuleRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, rule, remaining, expires_at FROM chaos_rules
         WHERE (remaining IS NULL OR remaining > 0)
           AND (expires_at IS NULL OR expires_at > ?1)
         ORDER BY rowid ASC",
    )?;
    let rows = stmt.query_map(params![now], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<i64>>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, rule, remaining, expires_at) = row?;
        out.push(ChaosRuleRow {
            id,
            rule: serde_json::from_str(&rule)?,
            remaining,
            expires_at,
        });
    }
    Ok(out)
}

/// Atomically consume one application of a rule: decrement `remaining` if
/// bounded, deleting the row when exhausted (spec §9: auto-delete).
pub fn consume_chaos_rule(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE chaos_rules SET remaining = remaining - 1
         WHERE id = ?1 AND remaining IS NOT NULL",
        params![id],
    )?;
    conn.execute(
        "DELETE FROM chaos_rules WHERE id = ?1 AND remaining IS NOT NULL AND remaining <= 0",
        params![id],
    )?;
    Ok(())
}

/// Delete expired rules (spec §9: expired rules auto-delete).
pub fn purge_expired_chaos(conn: &Connection, now: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM chaos_rules WHERE expires_at IS NOT NULL AND expires_at <= ?1",
        params![now],
    )?;
    Ok(())
}

/// Delete one chaos rule. Returns whether a row was removed.
pub fn delete_chaos_rule(conn: &Connection, id: &str) -> Result<bool> {
    let n = conn.execute("DELETE FROM chaos_rules WHERE id = ?1", params![id])?;
    Ok(n > 0)
}

/// Delete all chaos rules.
pub fn clear_chaos_rules(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM chaos_rules", [])?;
    Ok(())
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
