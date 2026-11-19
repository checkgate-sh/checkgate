//! A simulated SDK client that follows the shipped SDKs' streaming contract
//! (`sdks/nodejs/index.js` is the reference):
//!
//! - every (re)connect stages the replayed flag set and swaps it in on
//!   `ready`, so a replay cut short never goes live;
//! - change events (staged or live) apply only if they supersede the version
//!   held for that flag: newer revision, or a delete at the same revision;
//!   duplicates and older events are dropped;
//! - a closed or failed stream reconnects with exponential backoff and
//!   jitter, and after `poll_fallback_threshold` consecutive failures the
//!   client also polls `GET /flags/snapshot?v=2` (flags with revisions and
//!   tombstones) until a stream connects again;
//! - a complete set (the replay on `ready`, or a poll) replaces what the
//!   client learned before it was requested, and is merged per flag, newer
//!   version winning, with what it learned after (R20; `merge_full_set`
//!   mirrors the SDKs' `mergeFullSet`);
//! - an attempt that has not seen `connected` within `connect_timeout` is
//!   abandoned, and a connected stream that announced `heartbeat_ms` is
//!   abandoned after `idle_heartbeats` intervals without a complete frame
//!   (servers that announce nothing get no idle deadline);
//! - HTTP 401/403, from the stream or a poll, is terminal: the client stops.
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
    /// From the start of an attempt until `connected`.
    pub connect_timeout: Duration,
    /// Heartbeat intervals a connected stream may stay silent.
    pub idle_heartbeats: u32,
}

impl Default for SdkOptions {
    fn default() -> Self {
        Self {
            reconnect_base: Duration::from_millis(200),
            reconnect_max: Duration::from_secs(2),
            poll_fallback_threshold: 3,
            poll_interval: Duration::from_millis(500),
            poll_base: None,
            connect_timeout: Duration::from_secs(5),
            idle_heartbeats: 3,
        }
    }
}

/// A flag's last known revision, whether that revision deleted it, and the
/// learn counter value when the client applied it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Known {
    pub revision: i64,
    pub deleted: bool,
    pub at: u64,
}

impl Known {
    /// A delete at revision `r` supersedes the flag at `r`; anything newer
    /// supersedes both (the server's `sync::Version` order).
    fn supersedes(&self, other: &Known) -> bool {
        (self.revision, self.deleted) > (other.revision, other.deleted)
    }
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
    /// The interval the server announced on the last `connected`.
    pub heartbeat_ms: Option<u64>,
    pub heartbeats: u64,
    /// Stream attempts made, and how many a deadline abandoned.
    pub attempts: u32,
    pub deadline_trips: u32,
    /// Set when the server rejected the key: the client has stopped.
    pub auth_rejected: Option<u16>,
    staged: Option<(HashMap<String, Value>, HashMap<String, Known>)>,
    /// Grows with every applied change (see `merge_full_set`).
    learn_seq: u64,
    /// `learn_seq` when the current stream attempt was sent.
    stream_started_at: u64,
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

fn is_stale(known: Option<&Known>, incoming: Option<&Known>) -> bool {
    match (known, incoming) {
        (Some(k), Some(i)) => !i.supersedes(k),
        _ => false,
    }
}

/// The state after a complete set requested when `learn_seq` was
/// `started_at`: what was learned before is replaced (absences included),
/// what was learned after is kept unless the set has a newer version.
/// `incoming_revisions` is `None` for a server that sends none.
fn merge_full_set(
    state: &mut SdkState,
    incoming: HashMap<String, Value>,
    incoming_revisions: Option<HashMap<String, Known>>,
    started_at: u64,
) {
    state.learn_seq += 1;
    let at = state.learn_seq;
    let Some(offered_revisions) = incoming_revisions else {
        state.flags = incoming;
        state.revisions.clear();
        return;
    };
    let mut keys: HashSet<String> = incoming.keys().cloned().collect();
    keys.extend(offered_revisions.keys().cloned());
    keys.extend(state.revisions.keys().cloned());
    let mut flags = HashMap::new();
    let mut revisions = HashMap::new();
    for key in keys {
        let held = state.revisions.get(&key).copied();
        let offered = offered_revisions.get(&key).copied();
        if let Some(h) = held
            && h.at > started_at
            && offered.is_none_or(|o| !o.supersedes(&h))
        {
            revisions.insert(key.clone(), h);
            if let Some(flag) = state.flags.get(&key).filter(|_| !h.deleted) {
                flags.insert(key, flag.clone());
            }
            continue;
        }
        if let Some(o) = offered {
            revisions.insert(key.clone(), Known { at, ..o });
        }
        if let Some(flag) = incoming
            .get(&key)
            .filter(|_| !offered.is_some_and(|o| o.deleted))
        {
            flags.insert(key, flag.clone());
        }
    }
    state.flags = flags;
    state.revisions = revisions;
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
        at: 0,
    });

    if let Some((flags, revisions)) = state.staged.as_mut() {
        if is_stale(revisions.get(&key), known.as_ref()) {
            return;
        }
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
    if is_stale(state.revisions.get(&key), known.as_ref()) {
        state.stale_dropped += 1;
        return;
    }
    if let Some(k) = known {
        state.learn_seq += 1;
        let at = state.learn_seq;
        state.revisions.insert(key.clone(), Known { at, ..k });
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
            let payload = serde_json::from_str::<Value>(data).ok();
            s.env_id = payload
                .as_ref()
                .and_then(|v| v["environment_id"].as_str().map(str::to_string));
            s.heartbeat_ms = payload
                .as_ref()
                .and_then(|v| v["heartbeat_ms"].as_u64())
                .filter(|ms| *ms > 0);
        }
        "heartbeat" => s.heartbeats += 1,
        "ready" => {
            if let Some((flags, revisions)) = s.staged.take() {
                let started_at = s.stream_started_at;
                merge_full_set(&mut s, flags, Some(revisions), started_at);
                s.ready = true;
                s.readies += 1;
            }
        }
        "update" => apply(&mut s, data),
        _ => {}
    }
}

/// How one `/stream` attempt ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Attempt {
    /// The server sent `connected`; the stream later ended or went silent.
    Connected,
    /// Refused, failed or abandoned before `connected`.
    Failed,
    /// HTTP 401/403: the key is rejected.
    Rejected(u16),
}

fn is_auth_failure(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 401 | 403)
}

/// Reads one `/stream` connection to its end, or until it misses a deadline.
async fn stream_once(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    opts: &SdkOptions,
    state: &Mutex<SdkState>,
) -> Attempt {
    {
        let mut s = state.lock().unwrap();
        s.attempts += 1;
        // The replay this attempt receives is read after this point.
        s.stream_started_at = s.learn_seq;
    }
    let mut deadline = tokio::time::Instant::now() + opts.connect_timeout;
    let sent = client
        .get(format!("{base}/stream"))
        .bearer_auth(key)
        .header("Accept", "text/event-stream")
        .send();
    let Ok(Ok(resp)) = tokio::time::timeout_at(deadline, sent).await else {
        return Attempt::Failed;
    };
    if is_auth_failure(resp.status()) {
        return Attempt::Rejected(resp.status().as_u16());
    }
    if !resp.status().is_success() {
        return Attempt::Failed;
    }
    let mut body = resp.bytes_stream();
    let mut buf = String::new();
    let (mut event, mut data) = (String::new(), String::new());
    let mut connected = false;
    let mut idle: Option<Duration> = None;
    loop {
        let Ok(next) = tokio::time::timeout_at(deadline, body.next()).await else {
            state.lock().unwrap().deadline_trips += 1;
            break;
        };
        let Some(Ok(chunk)) = next else { break };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim_end_matches('\r').to_string();
            buf.drain(..=nl);
            if line.is_empty() {
                // Any complete frame, comments included, is proof of life.
                if let Some(idle) = idle {
                    deadline = tokio::time::Instant::now() + idle;
                }
                if !event.is_empty() || !data.is_empty() {
                    on_frame(state, &event, &data);
                    if event == "connected" {
                        connected = true;
                        idle = state.lock().unwrap().heartbeat_ms.map(|ms| {
                            Duration::from_millis(ms).saturating_mul(opts.idle_heartbeats)
                        });
                        // No heartbeat announced: no idle deadline.
                        deadline = tokio::time::Instant::now()
                            + idle.unwrap_or(Duration::from_secs(24 * 3600));
                    }
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
    if connected {
        Attempt::Connected
    } else {
        Attempt::Failed
    }
}

/// Returns the status when the server rejected the key.
async fn poll_once(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    state: &Mutex<SdkState>,
) -> Option<u16> {
    let started_at = state.lock().unwrap().learn_seq;
    let Ok(resp) = client
        .get(format!("{base}/flags/snapshot?v=2"))
        .bearer_auth(key)
        .timeout(Duration::from_secs(5))
        .send()
        .await
    else {
        return None;
    };
    if is_auth_failure(resp.status()) {
        return Some(resp.status().as_u16());
    }
    if !resp.status().is_success() {
        return None;
    }
    let Ok(body) = resp.json::<Value>().await else {
        return None;
    };
    let (flags, revisions) = parse_snapshot(body)?;
    let mut s = state.lock().unwrap();
    merge_full_set(&mut s, flags, revisions, started_at);
    s.polls += 1;
    s.ready = true;
    None
}

type FullSet = (HashMap<String, Value>, Option<HashMap<String, Known>>);

/// A `?v=2` snapshot (flags, revisions, tombstones), or a bare array from a
/// server that predates it. `None` if malformed: nothing is applied.
fn parse_snapshot(body: Value) -> Option<FullSet> {
    let keyed = |flags: Vec<Value>| -> HashMap<String, Value> {
        flags
            .into_iter()
            .filter_map(|f| Some((f["key"].as_str()?.to_string(), f)))
            .collect()
    };
    if let Value::Array(flags) = body {
        return Some((keyed(flags), None));
    }
    let flags = keyed(body["flags"].as_array()?.clone());
    let mut revisions = HashMap::new();
    for key in flags.keys() {
        let revision = body["revisions"][key].as_i64()?;
        revisions.insert(
            key.clone(),
            Known {
                revision,
                deleted: false,
                at: 0,
            },
        );
    }
    for (key, revision) in body["tombstones"].as_object()? {
        let revision = revision.as_i64()?;
        revisions.entry(key.clone()).or_insert(Known {
            revision,
            deleted: true,
            at: 0,
        });
    }
    Some((flags, Some(revisions)))
}

async fn run(base: String, key: String, opts: SdkOptions, state: Arc<Mutex<SdkState>>) {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let mut failures: u32 = 0;
    loop {
        let attempt = stream_once(&client, &base, &key, &opts, &state).await;
        {
            let mut s = state.lock().unwrap();
            s.staged = None;
            if s.streaming {
                s.disconnects += 1;
            }
            s.streaming = false;
            if let Attempt::Rejected(status) = attempt {
                s.auth_rejected = Some(status);
                s.polling = false;
                return;
            }
        }
        failures = if attempt == Attempt::Connected {
            1
        } else {
            failures + 1
        };

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
                if let Some(status) = poll_once(&client, poll_base, &key, &state).await {
                    let mut s = state.lock().unwrap();
                    s.auth_rejected = Some(status);
                    s.polling = false;
                    return;
                }
                let left = wake.saturating_duration_since(tokio::time::Instant::now());
                tokio::time::sleep(opts.poll_interval.min(left)).await;
            }
        }
        tokio::time::sleep_until(wake).await;
    }
}
