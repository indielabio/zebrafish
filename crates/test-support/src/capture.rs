//! [`CaptureServer`] — the webhook receiver test double (spec §16.4).
//!
//! A real axum listener on a random port. Tests point a zebrafish webhook
//! endpoint at [`CaptureServer::url`], then assert on what arrives:
//! [`expect_events`](CaptureServer::expect_events) (order-sensitive),
//! [`expect_no_events`](CaptureServer::expect_no_events), or raw
//! [`deliveries`](CaptureServer::deliveries) for signature verification.
//! Response codes can be scripted to exercise the retry schedule.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::any;
use serde_json::Value;

/// One captured webhook request.
#[derive(Debug, Clone)]
pub struct CapturedDelivery {
    /// The raw request body, byte-exact (what the signature covers).
    pub body: String,
    /// The `Stripe-Signature` header value, empty if absent.
    pub signature: String,
    /// The `Content-Type` header value, empty if absent.
    pub content_type: String,
}

impl CapturedDelivery {
    /// The event payload parsed as JSON.
    #[must_use]
    pub fn event(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }

    /// The event `type`, or `""`.
    #[must_use]
    pub fn event_type(&self) -> String {
        self.event()["type"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The event `id`, or `""`.
    #[must_use]
    pub fn event_id(&self) -> String {
        self.event()["id"].as_str().unwrap_or_default().to_string()
    }
}

#[derive(Debug)]
struct Inner {
    requests: Mutex<Vec<CapturedDelivery>>,
    /// Scripted response statuses, consumed front-first; empty → default.
    scripted: Mutex<VecDeque<u16>>,
    default_status: AtomicU16,
}

/// A capture server bound to a random local port.
#[derive(Debug, Clone)]
pub struct CaptureServer {
    addr: SocketAddr,
    inner: Arc<Inner>,
}

async fn capture(
    State(inner): State<Arc<Inner>>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, &'static str) {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    inner
        .requests
        .lock()
        .expect("capture mutex poisoned")
        .push(CapturedDelivery {
            body: String::from_utf8_lossy(&body).into_owned(),
            signature: header("stripe-signature"),
            content_type: header("content-type"),
        });
    let status = inner
        .scripted
        .lock()
        .expect("script mutex poisoned")
        .pop_front()
        .unwrap_or_else(|| inner.default_status.load(Ordering::SeqCst));
    (StatusCode::from_u16(status).unwrap_or(StatusCode::OK), "ok")
}

impl CaptureServer {
    /// Bind to a random port and start serving.
    ///
    /// # Panics
    /// Panics if no local port can be bound (test environment problem).
    pub async fn start() -> Self {
        let inner = Arc::new(Inner {
            requests: Mutex::new(Vec::new()),
            scripted: Mutex::new(VecDeque::new()),
            default_status: AtomicU16::new(200),
        });
        let router: Router = Router::new()
            .route("/", any(capture))
            .route("/{*path}", any(capture))
            .with_state(Arc::clone(&inner));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind capture server");
        let addr = listener.local_addr().expect("capture server addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self { addr, inner }
    }

    /// The URL to register as a webhook endpoint.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}/webhooks", self.addr)
    }

    /// Respond to every request with `status` (unless scripted).
    pub fn respond_with(&self, status: u16) {
        self.inner.default_status.store(status, Ordering::SeqCst);
    }

    /// Script the next responses, consumed in order; afterwards the default
    /// applies again.
    pub fn script_responses(&self, statuses: &[u16]) {
        self.inner
            .scripted
            .lock()
            .expect("script mutex poisoned")
            .extend(statuses.iter().copied());
    }

    /// Everything captured so far.
    #[must_use]
    pub fn deliveries(&self) -> Vec<CapturedDelivery> {
        self.inner
            .requests
            .lock()
            .expect("capture mutex poisoned")
            .clone()
    }

    /// Wait until at least `n` requests have been captured.
    ///
    /// # Panics
    /// Panics on timeout, listing what did arrive.
    pub async fn wait_for(&self, n: usize, timeout: Duration) -> Vec<CapturedDelivery> {
        let poll = Duration::from_millis(20);
        let mut waited = Duration::ZERO;
        loop {
            let got = self.deliveries();
            if got.len() >= n {
                return got;
            }
            if waited >= timeout {
                let types: Vec<String> = got.iter().map(CapturedDelivery::event_type).collect();
                panic!(
                    "expected {n} webhook deliveries within {timeout:?}, got {}: {types:?}",
                    got.len()
                );
            }
            tokio::time::sleep(poll).await;
            waited += poll;
        }
    }

    /// Order-sensitive event assertion (spec §16.4): wait for exactly
    /// `types.len()` deliveries and assert their `type` sequence.
    ///
    /// # Panics
    /// Panics on timeout or on any type/order mismatch.
    pub async fn expect_events(&self, types: &[&str], timeout: Duration) -> Vec<CapturedDelivery> {
        let got = self.wait_for(types.len(), timeout).await;
        let actual: Vec<String> = got.iter().map(CapturedDelivery::event_type).collect();
        assert_eq!(
            actual,
            types.iter().map(|s| (*s).to_string()).collect::<Vec<_>>(),
            "webhook event types/order mismatch"
        );
        got
    }

    /// Assert that nothing arrives for `window` (beyond what already has).
    ///
    /// # Panics
    /// Panics if a new delivery lands inside the window.
    pub async fn expect_no_events(&self, window: Duration) {
        let before = self.deliveries().len();
        tokio::time::sleep(window).await;
        let after = self.deliveries();
        assert_eq!(
            after.len(),
            before,
            "expected no further deliveries; got {:?}",
            after[before..]
                .iter()
                .map(CapturedDelivery::event_type)
                .collect::<Vec<_>>()
        );
    }
}
