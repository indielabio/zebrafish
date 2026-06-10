//! The webhook delivery worker (spec §8).
//!
//! A single tokio task fed post-commit by the [`zebrafish_core::World`] event
//! sink (an mpsc
//! channel — never the lossy broadcast bus). For each committed event it fans
//! out to every registered endpoint whose filter matches, signs the payload
//! with the endpoint's secret at the current virtual time, POSTs with a 10 s
//! timeout, and records every attempt in the `deliveries` table.
//!
//! Retries: non-2xx or connection failure ⇒ +5 s, +30 s, +2 m, then failed
//! (4 attempts total). Each retry is scheduled twice — a wall-clock sleep
//! (virtual time never advances on its own) *and* a virtual due-time drained
//! synchronously by `POST /_config/clock/advance`, whichever comes first
//! (spec §8 "virtual-time-aware").
//!
//! Webhook-side chaos (spec §9): `webhook_drop`, `webhook_duplicate`,
//! `webhook_delay`, `webhook_reorder` are applied here, matched by
//! `match.event_type` glob, consuming `times` once per (event, endpoint).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use zebrafish_core::clock::WallStopwatch;
use zebrafish_core::event::endpoint_filter_matches;
use zebrafish_core::store::DeliveryRow;
use zebrafish_core::{Result as CoreResult, StripeEvent};

use crate::chaos::glob_match;
use crate::state::AppState;
use crate::webhooks::sign::stripe_signature;

/// Per-attempt HTTP timeout (spec §8).
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);

/// Total attempts per (event, endpoint): the initial one plus three retries.
const MAX_ATTEMPTS: i64 = 4;

/// Seconds until the next attempt, given the attempt number being scheduled.
fn retry_delay_secs(next_attempt: i64) -> i64 {
    match next_attempt {
        2 => 5,
        3 => 30,
        _ => 120,
    }
}

/// One delivery unit: an event payload bound for one endpoint. `body` is the
/// exact byte string sent (and signed) on every attempt.
#[derive(Debug, Clone)]
struct Job {
    event_id: String,
    body: String,
    endpoint_id: String,
    attempt: i64,
}

/// A retry waiting for its due time (wall sleep or virtual drain).
#[derive(Debug)]
struct PendingRetry {
    job: Job,
    due_virtual: i64,
}

/// Why a manual redelivery failed.
#[derive(Debug)]
pub enum RedeliverError {
    /// The delivery worker is not running (in-process test router).
    NotRunning,
    /// Unknown event id.
    NoSuchEvent,
    /// No registered endpoint filter matches the event type.
    NoMatchingEndpoint,
}

/// Messages into the worker, from the [`DeliveryHandle`] and from the
/// worker's own timer tasks.
#[derive(Debug)]
pub enum Command {
    /// Fire every pending retry due at or before the current virtual time
    /// (sent by `POST /_config/clock/advance` after advancing).
    Drain { ack: oneshot::Sender<()> },
    /// Manually redeliver a stored event to all matching endpoints
    /// (`POST /_config/events/:id/redeliver`). One attempt, no auto-retries,
    /// chaos rules not applied — a manual action does exactly what it says.
    Redeliver {
        event_id: String,
        ack: oneshot::Sender<Result<Vec<Value>, RedeliverError>>,
    },
    /// A retry's wall-clock timer elapsed.
    FireRetry { key: u64 },
    /// A `webhook_reorder` window elapsed.
    FlushReorder { rule_id: String },
}

/// Cheaply-cloneable handle for talking to the worker.
#[derive(Debug, Clone)]
pub struct DeliveryHandle {
    tx: mpsc::UnboundedSender<Command>,
    running: Arc<AtomicBool>,
}

impl DeliveryHandle {
    /// Whether [`spawn_delivery_worker`] has been called.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Fire pending retries due at or before the current virtual time and
    /// wait for them to complete. A no-op when no worker is running.
    pub async fn drain(&self) {
        if !self.is_running() {
            return;
        }
        let (ack, done) = oneshot::channel();
        if self.tx.send(Command::Drain { ack }).is_ok() {
            let _ = done.await;
        }
    }

    /// Redeliver a stored event to all matching endpoints, returning the
    /// delivery-attempt rows.
    pub async fn redeliver(&self, event_id: &str) -> Result<Vec<Value>, RedeliverError> {
        if !self.is_running() {
            return Err(RedeliverError::NotRunning);
        }
        let (ack, done) = oneshot::channel();
        self.tx
            .send(Command::Redeliver {
                event_id: event_id.to_string(),
                ack,
            })
            .map_err(|_| RedeliverError::NotRunning)?;
        done.await.map_err(|_| RedeliverError::NotRunning)?
    }
}

/// The receiver halves created in `AppState::new`, consumed by
/// [`spawn_delivery_worker`].
#[derive(Debug)]
pub struct WorkerChannels {
    /// Post-commit events from the world sink.
    pub events: mpsc::UnboundedReceiver<StripeEvent>,
    /// Commands from the handle and the worker's own timers.
    pub commands: mpsc::UnboundedReceiver<Command>,
}

/// Build the channel plumbing: the sink sender to install on the world, the
/// handle for `AppState`, and the receivers for the worker.
#[must_use]
pub fn channels() -> (
    mpsc::UnboundedSender<StripeEvent>,
    DeliveryHandle,
    WorkerChannels,
) {
    let (event_tx, events) = mpsc::unbounded_channel();
    let (cmd_tx, commands) = mpsc::unbounded_channel();
    let handle = DeliveryHandle {
        tx: cmd_tx,
        running: Arc::new(AtomicBool::new(false)),
    };
    (event_tx, handle, WorkerChannels { events, commands })
}

/// Spawn the delivery worker onto the current tokio runtime. Returns `false`
/// if it was already spawned. Must be called for webhooks to deliver — the
/// binary does; in-process test routers may skip it.
pub fn spawn_delivery_worker(state: &AppState) -> bool {
    let Some(channels) = state.take_worker_channels() else {
        return false;
    };
    state.delivery.running.store(true, Ordering::SeqCst);
    let worker = Worker {
        state: state.clone(),
        client: reqwest::Client::new(),
        self_tx: state.delivery.tx.clone(),
        pending: HashMap::new(),
        next_key: 0,
        reorder: HashMap::new(),
    };
    tokio::spawn(worker.run(channels));
    true
}

/// What the chaos webhook rules said to do with one job.
#[derive(Debug, Default)]
struct WebhookEffects {
    drop: bool,
    duplicates: i64,
    delay_ms: Option<i64>,
    reorder: Option<(String, i64)>, // (rule id, window_ms)
}

struct Worker {
    state: AppState,
    client: reqwest::Client,
    self_tx: mpsc::UnboundedSender<Command>,
    pending: HashMap<u64, PendingRetry>,
    next_key: u64,
    /// Buffered jobs per `webhook_reorder` rule id, in arrival order.
    reorder: HashMap<String, Vec<Job>>,
}

impl Worker {
    async fn run(mut self, mut ch: WorkerChannels) {
        loop {
            tokio::select! {
                event = ch.events.recv() => match event {
                    Some(event) => self.on_event(event).await,
                    None => break,
                },
                cmd = ch.commands.recv() => match cmd {
                    Some(Command::Drain { ack }) => {
                        self.drain_due().await;
                        let _ = ack.send(());
                    }
                    Some(Command::Redeliver { event_id, ack }) => {
                        let _ = ack.send(self.redeliver(&event_id).await);
                    }
                    Some(Command::FireRetry { key }) => {
                        if let Some(p) = self.pending.remove(&key) {
                            self.attempt(p.job).await;
                        }
                    }
                    Some(Command::FlushReorder { rule_id }) => {
                        self.flush_reorder(&rule_id).await;
                    }
                    None => break,
                },
            }
        }
    }

    /// Fan a committed event out to matching endpoints, applying webhook
    /// chaos.
    async fn on_event(&mut self, event: StripeEvent) {
        let Ok(body) = serde_json::to_string(&event) else {
            return;
        };
        // One world lock: endpoints + chaos resolution (consuming `times`).
        let planned: Vec<(Job, WebhookEffects)> = {
            let mut world = self.state.world();
            let endpoints = match world.list_webhook_endpoints() {
                Ok(e) => e,
                Err(_) => return,
            };
            endpoints
                .into_iter()
                .filter(|ep| endpoint_filter_matches(&ep.events, &event.type_))
                .map(|ep| {
                    let effects = webhook_effects(&mut world, &event.type_);
                    (
                        Job {
                            event_id: event.id.clone(),
                            body: body.clone(),
                            endpoint_id: ep.id,
                            attempt: 1,
                        },
                        effects,
                    )
                })
                .collect()
        };

        for (job, effects) in planned {
            if effects.drop {
                continue;
            }
            if let Some((rule_id, window_ms)) = effects.reorder {
                let buffer = self.reorder.entry(rule_id.clone()).or_default();
                buffer.push(job);
                if buffer.len() == 1 {
                    let tx = self.self_tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(
                            u64::try_from(window_ms).unwrap_or(0),
                        ))
                        .await;
                        let _ = tx.send(Command::FlushReorder { rule_id });
                    });
                }
                continue;
            }
            if let Some(ms) = effects.delay_ms {
                self.schedule(job, Duration::from_millis(u64::try_from(ms).unwrap_or(0)));
                continue;
            }
            // 1 + duplicates identical signed deliveries (spec §9
            // `webhook_duplicate`: the handler sees N extra identical copies).
            for _ in 0..=effects.duplicates.max(0) {
                self.attempt(job.clone()).await;
            }
        }
    }

    /// One delivery attempt; on failure, schedules the next retry until
    /// [`MAX_ATTEMPTS`].
    async fn attempt(&mut self, job: Job) {
        let Some((_, success)) = self.deliver(&job).await else {
            return; // endpoint deregistered since scheduling
        };
        if !success && job.attempt < MAX_ATTEMPTS {
            let next = Job {
                attempt: job.attempt + 1,
                ..job
            };
            let delay = retry_delay_secs(next.attempt);
            self.schedule(next, Duration::from_secs(u64::try_from(delay).unwrap_or(0)));
        }
    }

    /// Park a job until `delay` elapses on the wall clock — or until a clock
    /// advance drains it, whichever comes first.
    fn schedule(&mut self, job: Job, delay: Duration) {
        let due_virtual = {
            let world = self.state.world();
            // Sub-second delays round up so a 1 s virtual advance flushes them.
            world.now()
                + i64::try_from(delay.as_secs()).unwrap_or(i64::MAX).max(0)
                + i64::from(delay.subsec_nanos() > 0)
        };
        let key = self.next_key;
        self.next_key += 1;
        self.pending.insert(key, PendingRetry { job, due_virtual });
        let tx = self.self_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx.send(Command::FireRetry { key });
        });
    }

    /// Fire every pending job due at or before the current virtual time, in
    /// due order (then insertion order, for determinism).
    async fn drain_due(&mut self) {
        let now = self.state.world().now();
        let mut due: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, p)| p.due_virtual <= now)
            .map(|(k, _)| *k)
            .collect();
        due.sort_by_key(|k| (self.pending[k].due_virtual, *k));
        for key in due {
            if let Some(p) = self.pending.remove(&key) {
                self.attempt(p.job).await;
            }
        }
    }

    /// Release a reorder buffer in *reverse* arrival order (deterministic
    /// "reordering" — the point is that the app must not assume order).
    async fn flush_reorder(&mut self, rule_id: &str) {
        let Some(mut jobs) = self.reorder.remove(rule_id) else {
            return;
        };
        jobs.reverse();
        for job in jobs {
            self.attempt(job).await;
        }
    }

    /// Manual redelivery: one signed attempt per matching endpoint, attempt
    /// numbers continuing from the delivery log.
    async fn redeliver(&mut self, event_id: &str) -> Result<Vec<Value>, RedeliverError> {
        let jobs: Vec<Job> = {
            let world = self.state.world();
            let payload = world
                .get_event(event_id)
                .ok()
                .flatten()
                .ok_or(RedeliverError::NoSuchEvent)?;
            // Round-trip through StripeEvent so the body bytes use the same
            // field order as the original delivery (a Value would serialize
            // its keys alphabetically and change the signed bytes).
            let event: StripeEvent =
                serde_json::from_value(payload).map_err(|_| RedeliverError::NoSuchEvent)?;
            let event_type = event.type_.clone();
            let body = serde_json::to_string(&event).map_err(|_| RedeliverError::NoSuchEvent)?;
            world
                .list_webhook_endpoints()
                .map_err(|_| RedeliverError::NoSuchEvent)?
                .into_iter()
                .filter(|ep| endpoint_filter_matches(&ep.events, &event_type))
                .map(|ep| {
                    let attempt = world.next_attempt(event_id, &ep.id).unwrap_or(1);
                    Job {
                        event_id: event_id.to_string(),
                        body: body.clone(),
                        endpoint_id: ep.id,
                        attempt,
                    }
                })
                .collect()
        };
        if jobs.is_empty() {
            return Err(RedeliverError::NoMatchingEndpoint);
        }
        let mut rows = Vec::new();
        for job in jobs {
            if let Some((row, _)) = self.deliver(&job).await {
                rows.push(row);
            }
        }
        Ok(rows)
    }

    /// Sign, POST, and record one attempt. Returns the recorded row JSON and
    /// whether the app answered 2xx; `None` if the endpoint no longer exists.
    async fn deliver(&self, job: &Job) -> Option<(Value, bool)> {
        let (url, signature) = {
            let world = self.state.world();
            let endpoint = world
                .get_webhook_endpoint(&job.endpoint_id)
                .ok()
                .flatten()?;
            let t = world.now();
            let sig = stripe_signature(&endpoint.secret, t, job.body.as_bytes());
            (endpoint.url, sig)
        };

        let stopwatch = WallStopwatch::start();
        let response = self
            .client
            .post(&url)
            .header("Stripe-Signature", &signature)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(job.body.clone())
            .timeout(ATTEMPT_TIMEOUT)
            .send()
            .await;
        let (status_code, response_body) = match response {
            Ok(r) => {
                let status = i64::from(r.status().as_u16());
                (Some(status), r.text().await.ok())
            }
            Err(_) => (None, None), // connection failure / timeout (spec §4)
        };
        let duration_ms = stopwatch.elapsed_ms();

        let row = {
            let mut world = self.state.world();
            let row = DeliveryRow {
                id: world.new_delivery_id(),
                event_id: job.event_id.clone(),
                endpoint_id: job.endpoint_id.clone(),
                attempt: job.attempt,
                request_body: job.body.clone(),
                signature,
                status_code,
                response_body,
                duration_ms: Some(duration_ms),
                delivered_at: world.now(),
            };
            let _: CoreResult<()> = world.record_delivery(&row);
            row
        };
        let success = status_code.is_some_and(|s| (200..300).contains(&s));
        Some((row.to_json(), success))
    }
}

/// Resolve (and consume) the webhook chaos rules matching `event_type` into
/// one [`WebhookEffects`]. Rules apply in creation order; `webhook_drop`
/// short-circuits.
fn webhook_effects(world: &mut zebrafish_core::World, event_type: &str) -> WebhookEffects {
    let mut effects = WebhookEffects::default();
    let rules = world.list_chaos_rules().unwrap_or_default();
    for rule in rules {
        let kind = rule
            .rule
            .pointer("/action/kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !kind.starts_with("webhook_") {
            continue;
        }
        let pattern = rule
            .rule
            .pointer("/match/event_type")
            .and_then(Value::as_str)
            .unwrap_or("*");
        if !glob_match(pattern, event_type) {
            continue;
        }
        let _ = world.consume_chaos_rule(&rule.id);
        match kind {
            "webhook_drop" => {
                effects.drop = true;
                break;
            }
            "webhook_duplicate" => {
                effects.duplicates += rule
                    .rule
                    .pointer("/action/count")
                    .and_then(Value::as_i64)
                    .unwrap_or(1);
            }
            "webhook_delay" => {
                effects.delay_ms = rule.rule.pointer("/action/ms").and_then(Value::as_i64);
            }
            "webhook_reorder" => {
                let window = rule
                    .rule
                    .pointer("/action/window_ms")
                    .and_then(Value::as_i64)
                    .unwrap_or(1000);
                effects.reorder = Some((rule.id.clone(), window));
            }
            _ => {}
        }
    }
    effects
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_schedule_is_5s_30s_2m() {
        assert_eq!(retry_delay_secs(2), 5);
        assert_eq!(retry_delay_secs(3), 30);
        assert_eq!(retry_delay_secs(4), 120);
    }

    #[tokio::test]
    async fn handle_is_inert_until_spawned() {
        let (_event_tx, handle, _channels) = channels();
        assert!(!handle.is_running());
        handle.drain().await; // must not hang
        assert!(matches!(
            handle.redeliver("evt_x").await,
            Err(RedeliverError::NotRunning)
        ));
    }
}
