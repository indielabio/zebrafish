-- zebrafish SQLite schema (spec §4).
-- Stripe objects are NOT relationally modeled: each row stores the full
-- `api_state` JSON exactly as the API returns it. References are string ids
-- inside that JSON, resolved on read.

CREATE TABLE IF NOT EXISTS objects (
  id         TEXT PRIMARY KEY,        -- "cus_NffrFeUfNV2Hib"
  type       TEXT NOT NULL,           -- "customer"
  api_state  TEXT NOT NULL,           -- full JSON as returned by GET /v1/...
  created    INTEGER NOT NULL,        -- virtual-clock unix seconds
  deleted    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_objects_type ON objects(type);
CREATE INDEX IF NOT EXISTS idx_objects_customer
  ON objects(json_extract(api_state, '$.customer'));

CREATE TABLE IF NOT EXISTS events (
  id         TEXT PRIMARY KEY,        -- "evt_..."
  type       TEXT NOT NULL,
  payload    TEXT NOT NULL,           -- full Event JSON (data.object snapshot inside)
  created    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_created ON events(created);

CREATE TABLE IF NOT EXISTS webhook_endpoints (
  id         TEXT PRIMARY KEY,        -- "we_..."
  url        TEXT NOT NULL,
  secret     TEXT NOT NULL,           -- "whsec_..."
  events     TEXT NOT NULL DEFAULT '["*"]'  -- JSON array of type filters
);

CREATE TABLE IF NOT EXISTS deliveries (
  id            TEXT PRIMARY KEY,     -- "del_..."
  event_id      TEXT NOT NULL,
  endpoint_id   TEXT NOT NULL,
  attempt       INTEGER NOT NULL,
  request_body  TEXT NOT NULL,
  signature     TEXT NOT NULL,
  status_code   INTEGER,              -- NULL = connection failure
  response_body TEXT,
  duration_ms   INTEGER,
  delivered_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_deliveries_event ON deliveries(event_id);

CREATE TABLE IF NOT EXISTS chaos_rules (
  id         TEXT PRIMARY KEY,
  rule       TEXT NOT NULL,           -- JSON, see §9
  remaining  INTEGER,                 -- NULL = unlimited
  expires_at INTEGER
);

CREATE TABLE IF NOT EXISTS world (    -- exactly one row
  id INTEGER PRIMARY KEY CHECK (id = 1),
  now_unix   INTEGER NOT NULL,        -- virtual clock
  seed       INTEGER NOT NULL,
  rng_state  BLOB NOT NULL,           -- serialized ChaCha state for cross-restart determinism
  stripe_api_version TEXT NOT NULL
);
