//! A simulated SDK client that follows the shipped SDKs' streaming contract
//! (`sdks/nodejs/index.js` is the reference):
//!
//! - every (re)connect stages the replayed flag set and swaps it in on
//!   `ready`, so a replay cut short never goes live;
//! - after `ready`, change events apply unless they are older than the
//!   revision held for that flag, and after a delete only a strictly newer
//!   upsert brings a flag back;
//! - a closed or failed stream reconnects with exponential backoff and
//!   jitter, and after `poll_fallback_threshold` consecutive failures the
//!   client also polls `GET /flags/snapshot` (which carries no revisions, so
//!   it clears them, as the Node SDK does) until a stream connects again.
//!
//! Delays are shortened so tests run quickly; the logic is unchanged.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use futures_util::StreamExt;
use rand::RngExt as _;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct SdkOptions {
    pub reconnect_base: Duration,
    pub reconnect_max: Duration,
    pub poll_fallback_threshold: u32,
    pub poll_interval: Duration,
    /// Where snapshots are polled, if not the stream's server (a proxy may
    /// block only long-lived connections).
    pub poll_base: Option<String>,
}

impl Default for SdkOptions {
    fn default() -> Self {
        Self {
            reconnect_base: Duration::from_millis(200),
            reconnect_max: Duration::from_secs(2),
            poll_fallback_threshold: 3,
            poll_interval: Duration::from_millis(500),
            poll_base: None,
        }
    }
}

/// A flag's last known revision, and whether that revision deleted it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Known {
    pub revision: i64,
    pub deleted: bool,
}

#[derive(Debug, Default)]
pub struct SdkState {
    /// Live flag set: key → flag JSON as delivered.
    pub flags: HashMap<String, Value>,
    pub revisions: HashMap<String, Known>,
    pub env_id: Option<String>,
    pub ready: bool,
    pub streaming: bool,
    pub polling: bool,
    pub connects: u32,
    pub readies: u32,
    pub disconnects: u32,
    pub polls: u32,
    /// Change events applied or dropped as stale, after `ready`.
    pub live_events: u64,
    pub stale_dropped: u64,
    /// Every `env_id` a change event was received for.
    pub seen_envs: HashSet<String>,
    staged: Option<(HashMap<String, Value>, HashMap<String, Known>)>,
}

pub struct SimSdk {
    pub name: String,
    pub state: Arc<Mutex<SdkState>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for SimSdk {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl SimSdk {
    pub fn start(name: &str, base: &str, sdk_key: &str) -> Self {
        Self::start_with(name, base, sdk_key, SdkOptions::default())
    }

    pub fn start_with(name: &str, base: &str, sdk_key: &str, opts: SdkOptions) -> Self {
        let state = Arc::new(Mutex::new(SdkState::default()));
        let task = tokio::spawn(run(
            base.to_string(),
            sdk_key.to_string(),
            opts,
            Arc::clone(&state),
        ));
        SimSdk {
            name: name.to_string(),
            state,
            task,
        }
    }

    pub fn snapshot(&self) -> (HashMap<String, Value>, HashMap<String, Known>) {
        let s = self.state.lock().unwrap();
        (s.flags.clone(), s.revisions.clone())
    }

    pub fn with<R>(&self, f: impl FnOnce(&SdkState) -> R) -> R {
        f(&self.state.lock().unwrap())
    }

    /// Waits until the client has completed at least `n` bootstraps.
    pub async fn wait_readies(&self, n: u32, within: Duration) {
        let deadline = tokio::time::Instant::now() + within;
        while self.with(|s| s.readies) < n {
            assert!(
                tokio::time::Instant::now() < deadline,
                "{}: expected {n} completed bootstraps within {within:?}, saw {}",
                self.name,
                self.with(|s| s.readies)
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

fn is_stale(known: Option<&Known>, revision: Option<i64>, is_upsert: bool) -> bool {
    match (known, revision) {
        (Some(k), Some(r)) if is_upsert && k.deleted => r <= k.revision,
        (Some(k), Some(r)) => r < k.revision,
        _ => false,
    }
}

fn apply(state: &mut SdkState, data: &str) {
    let Ok(event) = serde_json::from_str::<Value>(data) else {
        return;
    };
    if let Some(env) = event["env_id"].as_str() {
        state.seen_envs.insert(env.to_string());
    }
    let is_upsert = event["type"] == "UPSERT";
    if !is_upsert && event["type"] != "DELETE" {
        return;
    }
    let key = if is_upsert {
        event["flag"]["key"].as_str()
    } else {
        event["key"].as_str()
    };
    let Some(key) = key.map(str::to_string) else {
        return;
    };
    let revision = event["revision"].as_i64();
    let known = revision.map(|revision| Known {
        revision,
        deleted: !is_upsert,
    });

    if let Some((flags, revisions)) = state.staged.as_mut() {
        if is_upsert {
            flags.insert(key.clone(), event["flag"].clone());
        } else {
            flags.remove(&key);
        }
        if let Some(k) = known {
            revisions.insert(key, k);
        }
        return;
    }

    state.live_events += 1;
    if is_stale(state.revisions.get(&key), revision, is_upsert) {
        state.stale_dropped += 1;
        return;
    }
    if let Some(k) = known {
        state.revisions.insert(key.clone(), k);
    }
    if is_upsert {
        state.flags.insert(key, event["flag"].clone());
    } else {
        state.flags.remove(&key);
    }
}

fn on_frame(state: &Mutex<SdkState>, event: &str, data: &str) {
    let mut s = state.lock().unwrap();
    match event {
        "connected" => {
            s.staged = Some((HashMap::new(), HashMap::new()));
            s.connects += 1;
            s.streaming = true;
            s.polling = false;
            s.env_id = serde_json::from_str::<Value>(data)
                .ok()
                .and_then(|v| v["environment_id"].as_str().map(str::to_string));
        }
        "ready" => {
            if let Some((flags, revisions)) = s.staged.take() {
                s.flags = flags;
                s.revisions = revisions;
                s.ready = true;
                s.readies += 1;
            }
        }
        "update" => apply(&mut s, data),
        _ => {}
    }
}

/// Reads one `/stream` connection to its end. Returns whether the server
/// accepted it (sent `connected`).
async fn stream_once(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    state: &Mutex<SdkState>,
) -> bool {
    let Ok(resp) = client
        .get(format!("{base}/stream"))
        .bearer_auth(key)
        .header("Accept", "text/event-stream")
        .send()
        .await
    else {
        return false;
    };
    if !resp.status().is_success() {
        return false;
    }
    let mut body = resp.bytes_stream();
    let mut buf = String::new();
    let (mut event, mut data) = (String::new(), String::new());
    let mut connected = false;
    while let Some(Ok(chunk)) = body.next().await {
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim_end_matches('\r').to_string();
            buf.drain(..=nl);
            if line.is_empty() {
                if !event.is_empty() || !data.is_empty() {
                    connected |= event == "connected";
                    on_frame(state, &event, &data);
                    event.clear();
                    data.clear();
                }
            } else if let Some(v) = line.strip_prefix("event:") {
                event = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("data:") {
                data.push_str(v.trim());
            }
        }
    }
    connected
}

async fn poll_once(client: &reqwest::Client, base: &str, key: &str, state: &Mutex<SdkState>) {
    let Ok(resp) = client
        .get(format!("{base}/flags/snapshot"))
        .bearer_auth(key)
        .timeout(Duration::from_secs(5))
        .send()
        .await
    else {
        return;
    };
    if !resp.status().is_success() {
        return;
    }
    let Ok(flags) = resp.json::<Vec<Value>>().await else {
        return;
    };
    let mut s = state.lock().unwrap();
    s.flags = flags
        .into_iter()
        .filter_map(|f| Some((f["key"].as_str()?.to_string(), f)))
        .collect();
    s.revisions.clear();
    s.polls += 1;
    s.ready = true;
}

async fn run(base: String, key: String, opts: SdkOptions, state: Arc<Mutex<SdkState>>) {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let mut failures: u32 = 0;
    loop {
        let connected = stream_once(&client, &base, &key, &state).await;
        {
            let mut s = state.lock().unwrap();
            s.staged = None;
            if s.streaming {
                s.disconnects += 1;
            }
            s.streaming = false;
        }
        failures = if connected { 1 } else { failures + 1 };

        let base_delay = opts
            .reconnect_base
            .saturating_mul(2u32.saturating_pow(failures.saturating_sub(1)))
            .min(opts.reconnect_max);
        let jitter = base_delay.mul_f64(rand::rng().random_range(0.0..0.3));
        let wake = tokio::time::Instant::now() + base_delay + jitter;

        if failures >= opts.poll_fallback_threshold {
            state.lock().unwrap().polling = true;
            while tokio::time::Instant::now() < wake {
                let poll_base = opts.poll_base.as_deref().unwrap_or(&base);
                poll_once(&client, poll_base, &key, &state).await;
                let left = wake.saturating_duration_since(tokio::time::Instant::now());
                tokio::time::sleep(opts.poll_interval.min(left)).await;
            }
        }
        tokio::time::sleep_until(wake).await;
    }
}
