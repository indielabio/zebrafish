//! [`Zebrafish`] — the out-of-process spawner (spec §16.1, §16.4).
//!
//! Spawns the real binary (`env!("CARGO_BIN_EXE_zebrafish")` from the server
//! crate's integration tests) against an ephemeral world on a random port,
//! waits for the `listening on http://...` line, and exposes a thin typed
//! client over the v1 and `/_config` planes.

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// Any `sk_test_*` credential is accepted (spec §1); tests use this one.
pub const TEST_API_KEY: &str = "sk_test_zebrafish";

/// Builder for a spawned emulator process.
#[derive(Debug)]
pub struct ZebrafishBuilder {
    bin: String,
    seed: Option<u64>,
    envs: Vec<(String, String)>,
    args: Vec<String>,
}

impl ZebrafishBuilder {
    /// Set the deterministic seed (defaults to 42).
    #[must_use]
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Add an environment variable (e.g. `ZEBRAFISH_WEBHOOK_URL`).
    #[must_use]
    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.envs.push((key.to_string(), value.to_string()));
        self
    }

    /// Add an extra CLI argument.
    #[must_use]
    pub fn arg(mut self, arg: &str) -> Self {
        self.args.push(arg.to_string());
        self
    }

    /// Spawn and wait until the server is listening.
    ///
    /// # Panics
    /// Panics if the binary fails to start or never prints its address.
    pub async fn spawn(self) -> Zebrafish {
        let mut cmd = Command::new(&self.bin);
        cmd.args(["--ephemeral", "--port", "0", "--host", "127.0.0.1"])
            .args(&self.args)
            .env("ZEBRAFISH_SEED", self.seed.unwrap_or(42).to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &self.envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn zebrafish binary");

        let stderr = child.stderr.take().expect("stderr piped");
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<String>();
        let log_writer = Arc::clone(&log);
        tokio::spawn(async move {
            let mut addr_tx = Some(addr_tx);
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(rest) = line.strip_prefix("listening on http://")
                    && let Some(addr) = rest.split_whitespace().next()
                    && let Some(tx) = addr_tx.take()
                {
                    let _ = tx.send(addr.to_string());
                }
                log_writer
                    .lock()
                    .expect("stderr log mutex poisoned")
                    .push(line);
            }
        });

        let addr = tokio::time::timeout(Duration::from_secs(30), addr_rx)
            .await
            .expect("zebrafish did not print its address within 30s")
            .expect("zebrafish exited before listening");

        Zebrafish {
            child,
            base_url: format!("http://{addr}"),
            client: reqwest::Client::new(),
            stderr_log: log,
        }
    }
}

/// A running emulator process. Killed on drop.
#[derive(Debug)]
pub struct Zebrafish {
    #[allow(dead_code)] // held for kill_on_drop
    child: Child,
    /// `http://127.0.0.1:<port>`.
    pub base_url: String,
    client: reqwest::Client,
    stderr_log: Arc<Mutex<Vec<String>>>,
}

impl Zebrafish {
    /// Start building a spawn of the binary at `bin`
    /// (pass `env!("CARGO_BIN_EXE_zebrafish")`).
    #[must_use]
    pub fn builder(bin: &str) -> ZebrafishBuilder {
        ZebrafishBuilder {
            bin: bin.to_string(),
            seed: None,
            envs: Vec::new(),
            args: Vec::new(),
        }
    }

    /// Everything the process has written to stderr so far.
    #[must_use]
    pub fn stderr_lines(&self) -> Vec<String> {
        self.stderr_log
            .lock()
            .expect("stderr log mutex poisoned")
            .clone()
    }

    /// `POST` a form-encoded body to a `/v1` path with test credentials.
    pub async fn post_v1(&self, path: &str, form: &[(&str, &str)]) -> reqwest::Response {
        self.client
            .post(format!("{}{path}", self.base_url))
            .bearer_auth(TEST_API_KEY)
            .form(form)
            .send()
            .await
            .expect("v1 request")
    }

    /// `POST` a form-encoded body to a `/v1` path with extra headers
    /// (`Zebrafish-Fail` tests).
    pub async fn post_v1_with_headers(
        &self,
        path: &str,
        form: &[(&str, &str)],
        headers: &[(&str, &str)],
    ) -> reqwest::Response {
        let mut req = self
            .client
            .post(format!("{}{path}", self.base_url))
            .bearer_auth(TEST_API_KEY)
            .form(form);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        req.send().await.expect("v1 request")
    }

    /// `GET` a `/v1` path with test credentials, returning parsed JSON.
    pub async fn get_v1(&self, path: &str) -> Value {
        self.client
            .get(format!("{}{path}", self.base_url))
            .bearer_auth(TEST_API_KEY)
            .send()
            .await
            .expect("v1 request")
            .json()
            .await
            .expect("v1 response is JSON")
    }

    /// `POST` JSON to a `/_config` path.
    pub async fn config_post(&self, path: &str, body: Value) -> reqwest::Response {
        self.client
            .post(format!("{}{path}", self.base_url))
            .json(&body)
            .send()
            .await
            .expect("config request")
    }

    /// `GET` a `/_config` path, returning parsed JSON.
    pub async fn config_get(&self, path: &str) -> Value {
        self.client
            .get(format!("{}{path}", self.base_url))
            .send()
            .await
            .expect("config request")
            .json()
            .await
            .expect("config response is JSON")
    }

    /// `DELETE` a `/_config` path.
    pub async fn config_delete(&self, path: &str) -> reqwest::Response {
        self.client
            .delete(format!("{}{path}", self.base_url))
            .send()
            .await
            .expect("config request")
    }

    /// Register a webhook endpoint; returns `(endpoint_id, secret)`.
    pub async fn register_webhook(&self, url: &str) -> (String, String) {
        let res = self
            .config_post("/_config/webhooks", json!({ "url": url }))
            .await;
        assert!(res.status().is_success(), "webhook registration failed");
        let body: Value = res.json().await.expect("registration JSON");
        (
            body["id"].as_str().expect("endpoint id").to_string(),
            body["secret"]
                .as_str()
                .expect("endpoint secret")
                .to_string(),
        )
    }

    /// Install a chaos rule, returning its stored form (with `id`).
    pub async fn chaos(&self, rule: Value) -> Value {
        let res = self.config_post("/_config/chaos", rule).await;
        assert!(res.status().is_success(), "chaos rule rejected");
        res.json().await.expect("chaos rule JSON")
    }

    /// The current virtual time.
    pub async fn now(&self) -> i64 {
        self.config_get("/_config/clock").await["now"]
            .as_i64()
            .expect("clock now")
    }

    /// Advance the virtual clock by whole seconds (second-granularity retry
    /// tests); returns the advance report.
    pub async fn advance_secs(&self, secs: i64) -> Value {
        let target = self.now().await + secs;
        let res = self
            .config_post("/_config/clock/advance", json!({ "to_unix": target }))
            .await;
        assert!(res.status().is_success(), "clock advance failed");
        res.json().await.expect("advance JSON")
    }

    /// Advance the virtual clock by days.
    pub async fn advance_days(&self, days: i64) -> Value {
        let res = self
            .config_post("/_config/clock/advance", json!({ "days": days }))
            .await;
        assert!(res.status().is_success(), "clock advance failed");
        res.json().await.expect("advance JSON")
    }
}
