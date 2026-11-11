//! Cross-instance propagation of flag change events, and the paths that
//! recover SSE clients when an event is lost.
//!
//! Every flag write commits to PostgreSQL and then publishes a change event on
//! a Redis channel; each instance's subscriber relays the events to its local
//! SSE clients through a broadcast channel ([`relay`]). Redis Pub/Sub is
//! at-most-once: an event is lost if the publisher dies between commit and
//! publish, if Redis is unreachable, or if a subscriber's connection is down
//! or silently dead. The recovery paths, cheapest first:
//!
//! 1. A failed publish is delivered locally and a `RESYNC` broadcast is
//!    retried until Redis is back (`api::flags::publish_update`).
//! 2. A re-established subscription makes every local client re-bootstrap,
//!    since anything published meanwhile was missed ([`run_subscriber`]). A
//!    subscription that stops answering pings is treated as lost.
//! 3. Reconciliation ([`run_reconciler`]) is the backstop for everything the
//!    first two cannot see — above all a publisher that died after its commit,
//!    which leaves no instance holding the event. Each instance keeps a small
//!    index of the newest revision it has relayed for every flag
//!    ([`RelayIndex`]) and periodically compares it with the revisions
//!    committed in PostgreSQL. A committed change that has not been relayed
//!    within the grace period was lost; the instance then makes only the
//!    clients of that environment re-bootstrap.
//!
//! Guarantee: once PostgreSQL and the instance are reachable, a client
//! connected to the instance holds the committed state of its environment
//! within `SYNC_RECONCILE_INTERVAL_SECS × 1.2 + SYNC_RECONCILE_GRACE_SECS`
//! plus one reconnect (bounded staleness), whatever happened to the event.
//! Delivery is not exactly-once: events can be duplicated or reordered, and
//! SDKs drop stale ones by revision.

use crate::metrics::{Outcome, RedisDisconnect, ResyncTrigger};
use crate::state::AppState;
use rand::RngExt as _;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

pub const DEFAULT_CHANNEL: &str = "checkgate_updates";

static CHANNEL: OnceLock<String> = OnceLock::new();

/// The Redis channel change events travel on: `REDIS_CHANNEL`, so several
/// independent deployments (or test clusters) can share one Redis.
pub fn updates_channel() -> &'static str {
    CHANNEL.get_or_init(|| {
        std::env::var("REDIS_CHANNEL")
            .ok()
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| DEFAULT_CHANNEL.to_string())
    })
}

/// Payload telling every SSE handler to drop its clients so they reconnect and
/// re-bootstrap the full flag set from PostgreSQL.
pub const RESYNC_MESSAGE: &str = r#"{"type":"RESYNC"}"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Upsert,
    Delete,
    /// Re-bootstrap the clients of `env_id`, or every client when `None`.
    Resync,
    Other,
}

/// A flag's revision as an SDK orders it: a delete at revision `r` supersedes
/// the flag at `r`, and anything newer supersedes both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub revision: i64,
    pub deleted: bool,
}

/// One message on the local broadcast channel, parsed once on arrival rather
/// than once per SSE connection.
#[derive(Debug)]
pub struct SyncEvent {
    pub kind: EventKind,
    pub env_id: Option<String>,
    pub key: Option<String>,
    pub revision: Option<i64>,
    /// The JSON forwarded verbatim to SDKs as an `update` event.
    pub payload: String,
}

impl SyncEvent {
    pub fn parse(payload: String) -> Self {
        let value = serde_json::from_str::<serde_json::Value>(&payload).ok();
        let field = |name: &str| {
            value
                .as_ref()
                .and_then(|v| v.get(name))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        let kind = match field("type").as_deref() {
            Some("UPSERT") => EventKind::Upsert,
            Some("DELETE") => EventKind::Delete,
            Some("RESYNC") => EventKind::Resync,
            _ => EventKind::Other,
        };
        let key = match kind {
            EventKind::Upsert => value
                .as_ref()
                .and_then(|v| v.pointer("/flag/key"))
                .and_then(|k| k.as_str())
                .map(str::to_string),
            EventKind::Delete => field("key"),
            _ => None,
        };
        SyncEvent {
            kind,
            env_id: field("env_id"),
            key,
            revision: value
                .as_ref()
                .and_then(|v| v.get("revision"))
                .and_then(serde_json::Value::as_i64),
            payload,
        }
    }

    /// A resync of one environment's local clients. Never published to
    /// Redis: every instance reconciles its own clients.
    pub fn resync_env(env_id: String) -> Self {
        SyncEvent {
            kind: EventKind::Resync,
            payload: serde_json::json!({"type": "RESYNC", "env_id": env_id}).to_string(),
            env_id: Some(env_id),
            key: None,
            revision: None,
        }
    }

    pub fn resync_all() -> Self {
        SyncEvent {
            kind: EventKind::Resync,
            env_id: None,
            key: None,
            revision: None,
            payload: RESYNC_MESSAGE.to_string(),
        }
    }

    fn version(&self) -> Option<(&str, &str, Version)> {
        let deleted = match self.kind {
            EventKind::Upsert => false,
            EventKind::Delete => true,
            _ => return None,
        };
        Some((
            self.env_id.as_deref()?,
            self.key.as_deref()?,
            Version {
                revision: self.revision?,
                deleted,
            },
        ))
    }
}

/// Hands an event to this instance's SSE clients and records what was relayed.
pub fn relay(state: &AppState, event: SyncEvent) {
    if let Some((env, key, version)) = event.version() {
        state.sync.index.observe(env, key, version);
    } else if let (EventKind::Upsert | EventKind::Delete, Some(env), Some(key)) =
        (event.kind, event.env_id.as_deref(), event.key.as_deref())
    {
        // From a server that predates revisions (a rolling upgrade): the
        // change was relayed, its revision is simply unknown.
        state.sync.index.observe_unversioned(env, key);
    }
    if event.kind == EventKind::Resync && event.env_id.is_none() {
        // Every client re-bootstraps from PostgreSQL, so the index is
        // rebuilt from it too.
        state.sync.request_reseed();
    }
    if state.flag_tx.receiver_count() > 0 {
        let _ = state.flag_tx.send(Arc::new(event));
    }
}

// ── Configuration ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct SyncConfig {
    /// `None` disables reconciliation.
    pub reconcile_interval: Option<Duration>,
    pub reconcile_grace: Duration,
    pub ping_interval: Duration,
    pub ping_timeout: Duration,
    /// SSE clients told to resync close at a random point within this window,
    /// so thousands of them do not reconnect in the same instant.
    pub resync_spread: Duration,
}

impl SyncConfig {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let read = |name: &str, default: u64| -> Result<u64, String> {
            match lookup(name) {
                None => Ok(default),
                Some(v) => v
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| format!("{name} must be a non-negative integer, got {v:?}")),
            }
        };
        let interval = read("SYNC_RECONCILE_INTERVAL_SECS", 30)?;
        let grace = read("SYNC_RECONCILE_GRACE_SECS", 10)?;
        let ping = read("REDIS_PING_INTERVAL_SECS", 10)?;
        let spread = read("SSE_RESYNC_SPREAD_MS", 5_000)?;
        if ping == 0 {
            return Err("REDIS_PING_INTERVAL_SECS must be at least 1".into());
        }
        Ok(Self {
            reconcile_interval: (interval > 0).then(|| Duration::from_secs(interval)),
            reconcile_grace: Duration::from_secs(grace),
            ping_interval: Duration::from_secs(ping),
            ping_timeout: Duration::from_secs(ping.clamp(1, 5)),
            resync_spread: Duration::from_millis(spread),
        })
    }
}

pub struct SyncState {
    pub config: SyncConfig,
    pub index: RelayIndex,
    wake: Notify,
}

impl SyncState {
    pub fn new(config: SyncConfig) -> Arc<Self> {
        Arc::new(Self {
            index: RelayIndex::new(config.reconcile_interval.is_some()),
            config,
            wake: Notify::new(),
        })
    }

    /// Rebuild the index from PostgreSQL at the next pass, and run that pass
    /// now rather than at the next interval.
    pub fn request_reseed(&self) {
        self.index.unseed();
        self.wake.notify_one();
    }
}

// ── Relay index ──────────────────────────────────────────────────────────────

type EnvMap<V> = HashMap<String, HashMap<String, V>>;

/// The newest version of every flag this instance has relayed (or loaded from
/// PostgreSQL when seeding) — what a client that has received every event
/// holds. Memory is one entry per flag and recent tombstone.
pub struct RelayIndex {
    enabled: bool,
    inner: Mutex<IndexState>,
}

#[derive(Default)]
struct IndexState {
    seeded: bool,
    envs: EnvMap<Version>,
    /// Committed versions not yet relayed: (env, key) → (the committed
    /// version, when first noticed). Cleared once the index reaches it.
    suspects: HashMap<(String, String), (Version, Instant)>,
    /// Indexed environments that PostgreSQL no longer has, and since when.
    missing_envs: HashMap<String, Instant>,
    /// Flags relayed without a revision: take the committed version as
    /// relayed at the next pass.
    adopt: HashSet<(String, String)>,
}

/// What one reconciliation pass found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Findings {
    pub seeded: bool,
    /// Environments with committed changes not relayed within the grace
    /// period, with the number of affected flags and a few of their keys.
    pub diverged: Vec<(String, usize, Vec<String>)>,
    /// Environments that no longer exist.
    pub removed: Vec<String>,
}

impl RelayIndex {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            inner: Mutex::new(IndexState::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, IndexState> {
        // The state is only ever replaced or merged wholesale; a panic while
        // holding the lock cannot leave it in a state worse than stale.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn observe(&self, env: &str, key: &str, version: Version) {
        if !self.enabled {
            return;
        }
        let mut state = self.lock();
        let slot = state
            .envs
            .entry(env.to_string())
            .or_default()
            .entry(key.to_string())
            .or_insert(version);
        if version > *slot {
            *slot = version;
        }
    }

    pub fn observe_unversioned(&self, env: &str, key: &str) {
        if !self.enabled {
            return;
        }
        self.lock().adopt.insert((env.to_string(), key.to_string()));
    }

    fn unseed(&self) {
        self.lock().seeded = false;
    }

    pub fn tracked(&self) -> usize {
        self.lock().envs.values().map(HashMap::len).sum()
    }

    /// Compares the index with the committed state `db` (environments with no
    /// rows still appear in `live_envs`).
    pub fn reconcile(
        &self,
        db: &EnvMap<Version>,
        live_envs: &HashSet<String>,
        now: Instant,
        grace: Duration,
    ) -> Findings {
        let mut state = self.lock();
        let mut findings = Findings::default();
        if !state.seeded {
            let envs = std::mem::take(&mut state.envs);
            state.envs = db
                .iter()
                .map(|(env, keys)| (env.clone(), merge_ahead(keys, envs.get(env))))
                .collect();
            state.suspects.clear();
            state.missing_envs.clear();
            state.seeded = true;
            findings.seeded = true;
            return findings;
        }

        let empty = HashMap::new();
        let IndexState {
            envs,
            suspects,
            missing_envs,
            adopt,
            ..
        } = &mut *state;

        for (env, key) in adopt.drain() {
            if let Some(committed) = db.get(&env).and_then(|keys| keys.get(&key)) {
                let slot = envs
                    .entry(env)
                    .or_default()
                    .entry(key)
                    .or_insert(*committed);
                *slot = (*slot).max(*committed);
            }
        }

        // Environments that disappeared from PostgreSQL.
        missing_envs.retain(|env, _| !live_envs.contains(env) && envs.contains_key(env));
        for env in envs.keys() {
            if !live_envs.contains(env) {
                missing_envs.entry(env.clone()).or_insert(now);
            }
        }
        let gone: Vec<String> = missing_envs
            .iter()
            .filter(|(_, since)| now.saturating_duration_since(**since) >= grace)
            .map(|(env, _)| env.clone())
            .collect();
        for env in &gone {
            envs.remove(env);
            missing_envs.remove(env);
            suspects.retain(|(e, _), _| e != env);
        }
        findings.removed = gone;

        // Committed versions the index has not reached.
        let mut lagging: HashSet<(String, String)> = HashSet::new();
        for (env, keys) in db {
            let held = envs.get(env).unwrap_or(&empty);
            for (key, committed) in keys {
                if is_behind(held.get(key), committed) {
                    lagging.insert((env.clone(), key.clone()));
                    suspects
                        .entry((env.clone(), key.clone()))
                        .or_insert((*committed, now));
                }
            }
        }
        // Flags relayed as live that PostgreSQL has neither as a row nor as a
        // tombstone (deleted long ago, or an event this instance missed).
        for (env, keys) in envs.iter() {
            if !live_envs.contains(env) {
                continue;
            }
            let committed = db.get(env).unwrap_or(&empty);
            for (key, held) in keys {
                if !held.deleted && !committed.contains_key(key) {
                    lagging.insert((env.clone(), key.clone()));
                    let expected = Version {
                        revision: held.revision,
                        deleted: true,
                    };
                    suspects
                        .entry((env.clone(), key.clone()))
                        .or_insert((expected, now));
                }
            }
        }
        // A suspect clears once the index caught up with the version noticed
        // first; a newer write since then is a new suspect with its own clock.
        suspects.retain(|(env, key), (expected, since)| {
            let held = envs.get(env).and_then(|keys| keys.get(key));
            if !is_behind(held, expected) {
                if lagging.contains(&(env.clone(), key.clone())) {
                    *since = now;
                    if let Some(committed) = db.get(env).and_then(|keys| keys.get(key)) {
                        *expected = *committed;
                    }
                    return true;
                }
                return false;
            }
            true
        });

        let mut overdue: HashMap<String, Vec<String>> = HashMap::new();
        for ((env, key), (_, since)) in suspects.iter() {
            if now.saturating_duration_since(*since) >= grace {
                overdue.entry(env.clone()).or_default().push(key.clone());
            }
        }
        for (env, mut keys) in overdue {
            // Its clients re-bootstrap, so the committed state is what they hold.
            let held = envs.remove(&env);
            envs.insert(
                env.clone(),
                db.get(&env)
                    .map(|committed| merge_ahead(committed, held.as_ref()))
                    .unwrap_or_default(),
            );
            suspects.retain(|(e, _), _| *e != env);
            keys.sort();
            let count = keys.len();
            keys.truncate(5);
            findings.diverged.push((env, count, keys));
        }
        findings.diverged.sort();
        findings
    }
}

/// Whether a client holding `held` (nothing, for a flag it never saw) lacks
/// `committed`. Never having seen a flag is as good as holding its deletion.
fn is_behind(held: Option<&Version>, committed: &Version) -> bool {
    match held {
        None => !committed.deleted,
        Some(v) => v < committed,
    }
}

/// The committed versions, raised to any newer version already relayed
/// (an event can arrive after the snapshot it was compared with).
fn merge_ahead(
    committed: &HashMap<String, Version>,
    held: Option<&HashMap<String, Version>>,
) -> HashMap<String, Version> {
    committed
        .iter()
        .map(|(key, c)| {
            let v = held.and_then(|h| h.get(key)).map_or(*c, |h| (*h).max(*c));
            (key.clone(), v)
        })
        .collect()
}

// ── Reconciliation ───────────────────────────────────────────────────────────

/// Reads every committed flag revision and recent tombstone, plus the set of
/// environments, in one snapshot.
async fn load_committed(
    db: &sqlx::PgPool,
) -> Result<(EnvMap<Version>, HashSet<String>), sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let envs: Vec<String> = sqlx::query_scalar("SELECT id::text FROM environments")
        .fetch_all(&mut *tx)
        .await?;
    let rows: Vec<(String, String, i64, bool)> = sqlx::query_as(
        "SELECT environment_id::text, key, revision, false FROM flags \
         UNION ALL \
         SELECT t.environment_id::text, t.key, t.revision, true FROM flag_tombstones t \
         WHERE NOT EXISTS (SELECT 1 FROM flags f \
                           WHERE f.environment_id = t.environment_id AND f.key = t.key)",
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let live: HashSet<String> = envs.into_iter().collect();
    let mut committed: EnvMap<Version> = HashMap::new();
    for (env, key, revision, deleted) in rows {
        if live.contains(&env) {
            committed
                .entry(env)
                .or_default()
                .insert(key, Version { revision, deleted });
        }
    }
    Ok((committed, live))
}

/// One reconciliation pass: compare, then resync what diverged.
pub async fn reconcile_once(state: &AppState) -> Result<Findings, sqlx::Error> {
    let started = Instant::now();
    let result = load_committed(&state.db).await;
    let metrics = &state.metrics;
    metrics
        .reconcile_duration
        .observe(started.elapsed().as_secs_f64());
    let (committed, live) = match result {
        Ok(v) => v,
        Err(e) => {
            metrics.reconciled(Outcome::failure);
            metrics.db_up.set(0);
            return Err(e);
        }
    };
    metrics.reconciled(Outcome::success);
    metrics.db_up.set(1);
    let findings = state.sync.index.reconcile(
        &committed,
        &live,
        Instant::now(),
        state.sync.config.reconcile_grace,
    );
    if findings.seeded {
        debug!(
            tracked = state.sync.index.tracked(),
            "Sync index seeded from PostgreSQL"
        );
    }
    for (env, count, sample) in &findings.diverged {
        metrics.sync_divergences.inc();
        metrics.resync(ResyncTrigger::reconciliation);
        warn!(
            environment_id = %env,
            flags = count,
            sample_keys = ?sample,
            "Committed flag changes were never relayed to this instance — resyncing the environment's clients"
        );
        relay(state, SyncEvent::resync_env(env.clone()));
    }
    for env in &findings.removed {
        metrics.resync(ResyncTrigger::environment_removed);
        info!(environment_id = %env, "Environment no longer exists — closing its streams");
        relay(state, SyncEvent::resync_env(env.clone()));
    }
    Ok(findings)
}

/// Runs reconciliation every `SYNC_RECONCILE_INTERVAL_SECS` (±20 % jitter, so
/// replicas do not query in lockstep) until `cancel` fires.
pub async fn run_reconciler(state: AppState, cancel: CancellationToken) {
    let Some(interval) = state.sync.config.reconcile_interval else {
        info!("Sync reconciliation disabled (SYNC_RECONCILE_INTERVAL_SECS=0)");
        return;
    };
    loop {
        if let Err(e) = reconcile_once(&state).await {
            warn!(error = %e, "Sync reconciliation failed — will retry");
        }
        let jittered = interval.mul_f64(rand::rng().random_range(0.8..1.2));
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(jittered) => {}
            () = state.sync.wake.notified() => {}
        }
    }
}

// ── Redis subscriber ─────────────────────────────────────────────────────────

/// Keeps a subscription to the updates channel and relays every message.
/// `subscribed` fires after the first successful subscription.
pub async fn run_subscriber(
    state: AppState,
    redis: redis::Client,
    cancel: CancellationToken,
    subscribed: Arc<Notify>,
) {
    let cfg = state.sync.config.clone();
    let channel = updates_channel();
    let mut first = true;
    let retry = Duration::from_secs(2);
    loop {
        let connect = async {
            let pubsub = redis.get_async_pubsub().await?;
            let (mut sink, stream) = pubsub.split();
            sink.subscribe(channel).await?;
            Ok::<_, redis::RedisError>((sink, stream))
        };
        let attempt = tokio::select! {
            () = cancel.cancelled() => return,
            r = tokio::time::timeout(Duration::from_secs(5), connect) => r,
        };
        let (mut sink, mut stream) = match attempt {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => {
                state.metrics.redis_lost(RedisDisconnect::connect_failed);
                error!(error = %e, "Redis subscriber: connect/subscribe failed — retrying in 2s");
                if sleep_or_cancel(retry, &cancel).await {
                    return;
                }
                continue;
            }
            Err(_) => {
                state.metrics.redis_lost(RedisDisconnect::connect_failed);
                error!("Redis subscriber: connect/subscribe timed out — retrying in 2s");
                if sleep_or_cancel(retry, &cancel).await {
                    return;
                }
                continue;
            }
        };
        state.metrics.redis_subscriber_connected.set(1);
        info!(channel, "Redis subscriber: listening");
        if first {
            subscribed.notify_one();
        } else {
            // Anything published while the subscription was down never
            // reached this instance.
            warn!("Redis subscriber: resubscribed — resyncing local SSE clients");
            state.metrics.resync(ResyncTrigger::subscriber_reconnect);
            relay(&state, SyncEvent::resync_all());
        }
        first = false;

        // A connection can stay open while delivering nothing (a lost NAT or
        // load balancer mapping). A ping answered on the same connection
        // proves the subscription is still alive.
        let mut ping = tokio::time::interval(cfg.ping_interval);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ping.tick().await;
        let reason = loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    state.metrics.redis_subscriber_connected.set(0);
                    return;
                }
                msg = stream.next() => {
                    let Some(msg) = msg else { break RedisDisconnect::closed };
                    match msg.get_payload::<String>() {
                        Ok(payload) => {
                            state.metrics.flag_updates_received.inc();
                            let event = SyncEvent::parse(payload);
                            if event.kind == EventKind::Resync {
                                state.metrics.resync(ResyncTrigger::remote);
                                info!("Redis subscriber: resync broadcast received");
                            }
                            relay(&state, event);
                        }
                        Err(e) => warn!(error = %e, "Redis subscriber: undecodable payload"),
                    }
                }
                _ = ping.tick() => {
                    // A subscribed RESP2 connection answers ["pong", ""], not
                    // "PONG": any reply proves liveness.
                    match tokio::time::timeout(cfg.ping_timeout, sink.ping::<redis::Value>()).await {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => {
                            warn!(error = %e, "Redis subscriber: ping failed");
                            break RedisDisconnect::ping_timeout;
                        }
                        Err(_) => {
                            warn!(timeout = ?cfg.ping_timeout, "Redis subscriber: ping unanswered");
                            break RedisDisconnect::ping_timeout;
                        }
                    }
                }
            }
        };
        state.metrics.redis_subscriber_connected.set(0);
        state.metrics.redis_lost(reason);
        warn!(reason = ?reason, "Redis subscriber: connection lost — reconnecting");
    }
}

/// Sleeps, returning `true` if cancelled first.
pub async fn sleep_or_cancel(d: Duration, cancel: &CancellationToken) -> bool {
    tokio::select! {
        () = cancel.cancelled() => true,
        () = tokio::time::sleep(d) => false,
    }
}

/// Random delay within the configured resync spread.
pub fn resync_delay(spread: Duration) -> Duration {
    if spread.is_zero() {
        return Duration::ZERO;
    }
    spread.mul_f64(rand::rng().random_range(0.0..1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(revision: i64, deleted: bool) -> Version {
        Version { revision, deleted }
    }

    fn db(entries: &[(&str, &str, i64, bool)]) -> (EnvMap<Version>, HashSet<String>) {
        let mut map: EnvMap<Version> = HashMap::new();
        let mut envs = HashSet::new();
        for (env, key, rev, del) in entries {
            envs.insert(env.to_string());
            map.entry(env.to_string())
                .or_default()
                .insert(key.to_string(), v(*rev, *del));
        }
        (map, envs)
    }

    const G: Duration = Duration::from_secs(10);

    fn seeded(entries: &[(&str, &str, i64, bool)]) -> (RelayIndex, Instant) {
        let idx = RelayIndex::new(true);
        let (d, e) = db(entries);
        let t0 = Instant::now();
        assert!(idx.reconcile(&d, &e, t0, G).seeded);
        (idx, t0)
    }

    #[test]
    fn delete_outranks_the_revision_it_deleted() {
        assert!(v(5, true) > v(5, false));
        assert!(v(6, false) > v(5, true));
    }

    #[test]
    fn relayed_changes_are_not_divergence() {
        let (idx, t0) = seeded(&[("e", "a", 1, false)]);
        idx.observe("e", "a", v(2, false));
        idx.observe("e", "b", v(3, false));
        let (d, e) = db(&[("e", "a", 2, false), ("e", "b", 3, false)]);
        assert_eq!(idx.reconcile(&d, &e, t0 + G * 5, G), Findings::default());
    }

    #[test]
    fn a_change_not_relayed_within_grace_diverges() {
        let (idx, t0) = seeded(&[("e", "a", 1, false), ("x", "a", 1, false)]);
        let (d, e) = db(&[("e", "a", 2, false), ("x", "a", 1, false)]);
        // First sighting: maybe the event is still in flight.
        assert!(idx.reconcile(&d, &e, t0, G).diverged.is_empty());
        assert!(idx.reconcile(&d, &e, t0 + G / 2, G).diverged.is_empty());
        let f = idx.reconcile(&d, &e, t0 + G, G);
        assert_eq!(f.diverged, vec![("e".into(), 1, vec!["a".into()])]);
        // The environment was reseeded: nothing pending afterwards.
        assert!(idx.reconcile(&d, &e, t0 + G * 3, G).diverged.is_empty());
    }

    #[test]
    fn a_late_event_clears_the_suspicion() {
        let (idx, t0) = seeded(&[("e", "a", 1, false)]);
        let (d, e) = db(&[("e", "a", 2, false)]);
        assert!(idx.reconcile(&d, &e, t0, G).diverged.is_empty());
        idx.observe("e", "a", v(2, false));
        assert!(idx.reconcile(&d, &e, t0 + G * 2, G).diverged.is_empty());
    }

    #[test]
    fn a_busy_flag_is_judged_by_the_first_version_noticed() {
        // The flag keeps changing and each event arrives, but every pass
        // happens to catch the database one write ahead: no divergence.
        let (idx, t0) = seeded(&[("e", "a", 1, false)]);
        for n in 0..10i64 {
            let (d, e) = db(&[("e", "a", n + 2, false)]);
            let now = t0 + G / 2 * u32::try_from(n).unwrap();
            assert!(
                idx.reconcile(&d, &e, now, G).diverged.is_empty(),
                "pass {n}"
            );
            idx.observe("e", "a", v(n + 2, false));
        }
    }

    #[test]
    fn missed_delete_and_missed_create_diverge() {
        let (idx, t0) = seeded(&[("e", "gone", 4, false)]);
        let (d, e) = db(&[
            ("e", "gone", 4, true),
            ("e", "new", 9, false),
            // Created and deleted with both events lost: the client, which
            // never saw it, is already right.
            ("e", "brief", 8, true),
        ]);
        idx.reconcile(&d, &e, t0, G);
        let f = idx.reconcile(&d, &e, t0 + G, G);
        assert_eq!(
            f.diverged,
            vec![("e".into(), 2, vec!["gone".into(), "new".into()])]
        );
    }

    #[test]
    fn live_flag_absent_from_the_database_diverges() {
        // Deleted so long ago that its tombstone was pruned.
        let (idx, t0) = seeded(&[("e", "old", 4, false), ("e", "keep", 1, false)]);
        let (d, e) = db(&[("e", "keep", 1, false)]);
        idx.reconcile(&d, &e, t0, G);
        let f = idx.reconcile(&d, &e, t0 + G, G);
        assert_eq!(f.diverged, vec![("e".into(), 1, vec!["old".into()])]);
    }

    #[test]
    fn removed_environment_is_reported_once_after_grace() {
        let (idx, t0) = seeded(&[("e", "a", 1, false), ("dead", "a", 1, false)]);
        let (d, e) = db(&[("e", "a", 1, false)]);
        assert!(idx.reconcile(&d, &e, t0, G).removed.is_empty());
        let f = idx.reconcile(&d, &e, t0 + G, G);
        assert_eq!(f.removed, vec!["dead".to_string()]);
        assert!(f.diverged.is_empty());
        assert!(idx.reconcile(&d, &e, t0 + G * 2, G).removed.is_empty());
    }

    #[test]
    fn reseed_keeps_versions_relayed_after_the_snapshot() {
        let (idx, t0) = seeded(&[("e", "a", 1, false)]);
        idx.unseed();
        idx.observe("e", "a", v(7, false));
        let (d, e) = db(&[("e", "a", 5, false)]);
        assert!(idx.reconcile(&d, &e, t0, G).seeded);
        let (d, e) = db(&[("e", "a", 7, false)]);
        assert!(idx.reconcile(&d, &e, t0 + G * 2, G).diverged.is_empty());
    }

    #[test]
    fn unversioned_events_adopt_the_committed_revision() {
        let (idx, t0) = seeded(&[("e", "a", 1, false)]);
        idx.observe_unversioned("e", "a");
        let (d, e) = db(&[("e", "a", 2, false)]);
        assert!(idx.reconcile(&d, &e, t0, G).diverged.is_empty());
        assert!(idx.reconcile(&d, &e, t0 + G * 2, G).diverged.is_empty());
    }

    #[test]
    fn parse_extracts_routing_fields() {
        let e = SyncEvent::parse(
            r#"{"type":"UPSERT","env_id":"e1","flag":{"key":"k"},"revision":3}"#.into(),
        );
        assert_eq!(e.kind, EventKind::Upsert);
        assert_eq!(e.version(), Some(("e1", "k", v(3, false))));
        let e =
            SyncEvent::parse(r#"{"type":"DELETE","env_id":"e1","key":"k","revision":3}"#.into());
        assert_eq!(e.version(), Some(("e1", "k", v(3, true))));
        let e = SyncEvent::parse(RESYNC_MESSAGE.into());
        assert_eq!((e.kind, e.env_id), (EventKind::Resync, None));
        assert_eq!(SyncEvent::parse("not json".into()).kind, EventKind::Other);
    }

    #[test]
    fn config_defaults_and_validation() {
        let c = SyncConfig::from_lookup(|_| None).unwrap();
        assert_eq!(c.reconcile_interval, Some(Duration::from_secs(30)));
        assert_eq!(c.reconcile_grace, Duration::from_secs(10));
        assert_eq!(c.ping_timeout, Duration::from_secs(5));
        let off =
            SyncConfig::from_lookup(|n| (n == "SYNC_RECONCILE_INTERVAL_SECS").then(|| "0".into()))
                .unwrap();
        assert_eq!(off.reconcile_interval, None);
        assert!(
            SyncConfig::from_lookup(|n| (n == "REDIS_PING_INTERVAL_SECS").then(|| "0".into()))
                .is_err()
        );
        assert!(
            SyncConfig::from_lookup(|n| (n == "SSE_RESYNC_SPREAD_MS").then(|| "x".into())).is_err()
        );
    }
}
