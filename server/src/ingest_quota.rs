//! Per-credential quota on analytics ingestion, shared by every instance.
//!
//! The per-address limit in [`crate::rate_limit`] cannot stop one public SDK
//! key from being used by many addresses, and it counts requests, not rows: a
//! request may carry 500 rows. This quota is keyed by the credential that
//! authenticated the request (an SDK key, or the user behind a session or
//! personal access token) and charges two budgets per minute:
//!
//! - **requests**: one unit per ingest request;
//! - **rows**: one unit per row in the batch, or one per started KiB of
//!   request body when that is larger, so a few huge rows cost as much as
//!   the bytes they store.
//!
//! Both are generic cell-rate (GCRA) buckets with a burst of one minute's
//! budget, the same shape as the per-address limits. State lives in Redis
//! (one hash per credential that expires once its buckets have refilled), and
//! one Lua script checks and charges both atomically using the Redis clock, so
//! concurrent requests on any number of instances see one budget. A refused
//! request charges nothing.
//!
//! If Redis does not answer within [`REDIS_TIMEOUT`], the instance enforces
//! the same quota on its own for [`FALLBACK_HOLD`] before trying Redis again.
//! With *N* instances, a key can then reach up to *N* times its budget, never
//! more; flag delivery is not affected either way.

use crate::metrics::Metrics;
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter, clock::Clock};
use redis::aio::ConnectionManager;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tracing::warn;

/// Rows accepted in one ingest request (impressions or events).
pub const MAX_BATCH_ROWS: usize = 500;
/// Request-body bytes that count as one row of quota.
pub const BYTES_PER_UNIT: usize = 1024;
/// How long one quota decision may wait for Redis.
const REDIS_TIMEOUT: Duration = Duration::from_millis(500);
/// After Redis fails, decide locally this long before trying it again.
const FALLBACK_HOLD: Duration = Duration::from_secs(5);
/// The budget period; a full period's budget may be spent at once.
const PERIOD: Duration = Duration::from_secs(60);

/// Per-minute budgets for each credential. `None` disables that budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IngestQuotaConfig {
    pub requests: Option<NonZeroU32>,
    pub rows: Option<NonZeroU32>,
}

impl Default for IngestQuotaConfig {
    fn default() -> Self {
        Self {
            requests: NonZeroU32::new(20_000),
            rows: NonZeroU32::new(60_000),
        }
    }
}

impl IngestQuotaConfig {
    /// Reads `RATE_LIMIT_INGEST_KEY_PER_MINUTE` (requests) and
    /// `RATE_LIMIT_INGEST_ROWS_PER_MINUTE` (rows). Unset keeps the default,
    /// `0` disables the budget. A row budget must admit one full batch.
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let defaults = Self::default();
        let read = |name: &str, default: Option<NonZeroU32>| match lookup(name) {
            None => Ok(default),
            Some(v) => v
                .trim()
                .parse::<u32>()
                .map(NonZeroU32::new)
                .map_err(|_| format!("{name} must be a non-negative integer, got {v:?}")),
        };
        let config = Self {
            requests: read("RATE_LIMIT_INGEST_KEY_PER_MINUTE", defaults.requests)?,
            rows: read("RATE_LIMIT_INGEST_ROWS_PER_MINUTE", defaults.rows)?,
        };
        if let Some(rows) = config.rows
            && usize::try_from(rows.get()).unwrap_or(usize::MAX) < MAX_BATCH_ROWS
        {
            return Err(format!(
                "RATE_LIMIT_INGEST_ROWS_PER_MINUTE must be 0 (off) or at least {MAX_BATCH_ROWS}, \
                 one full batch; got {rows}"
            ));
        }
        Ok(config)
    }

    pub fn is_enabled(&self) -> bool {
        self.requests.is_some() || self.rows.is_some()
    }
}

/// The credential a request's ingestion is charged to.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum QuotaIdentity {
    /// An SDK key row id (`-1` is the legacy `SDK_KEY` variable).
    SdkKey(i64),
    /// A user, through a session or a personal access token.
    User(i64),
}

impl QuotaIdentity {
    fn redis_suffix(self) -> String {
        match self {
            QuotaIdentity::SdkKey(id) => format!("key:{id}"),
            QuotaIdentity::User(id) => format!("user:{id}"),
        }
    }
}

/// Which budget refused a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exceeded {
    Requests,
    Rows,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub exceeded: Exceeded,
    /// When the same request would next be accepted (at most one period).
    pub retry_after: Duration,
}

/// Row units a request is charged: its rows, or its body size in started
/// KiB when that is larger.
pub fn units_for(rows: usize, body_bytes: usize) -> u32 {
    let units = rows.max(body_bytes.div_ceil(BYTES_PER_UNIT));
    u32::try_from(units).unwrap_or(u32::MAX)
}

/// Checks and charges both buckets of one credential. Times are microseconds
/// from the Redis clock; the stored theoretical arrival times are written
/// with `%.0f` so they keep full precision (Lua's default number format has
/// 14 significant digits). Returns `{0, 0}` when accepted, or `{1|2, wait}`
/// (requests | rows) with the microseconds until it would be accepted.
const SCRIPT: &str = r"
if redis.replicate_commands then redis.replicate_commands() end
local t = redis.call('TIME')
local now = tonumber(t[1]) * 1000000 + tonumber(t[2])
local request_interval = tonumber(ARGV[1])
local row_interval = tonumber(ARGV[2])
local units = tonumber(ARGV[3])
local tolerance = tonumber(ARGV[4])
local function next_tat(field, interval, cost)
  if interval <= 0 then return nil end
  local tat = tonumber(redis.call('HGET', KEYS[1], field) or '0')
  if tat < now then tat = now end
  return tat + interval * cost
end
local r = next_tat('r', request_interval, 1)
local n = next_tat('n', row_interval, units)
if n and n - now > tolerance then
  return {2, math.ceil(n - now - tolerance)}
end
if r and r - now > tolerance then
  return {1, math.ceil(r - now - tolerance)}
end
local ttl = 0
if r then
  redis.call('HSET', KEYS[1], 'r', string.format('%.0f', r))
  ttl = r - now
end
if n then
  redis.call('HSET', KEYS[1], 'n', string.format('%.0f', n))
  if n - now > ttl then ttl = n - now end
end
redis.call('PEXPIRE', KEYS[1], math.ceil(ttl / 1000) + 1000)
return {0, 0}
";

type LocalLimiter = DefaultKeyedRateLimiter<QuotaIdentity>;

pub struct IngestQuota {
    config: IngestQuotaConfig,
    /// Redis key prefix: the deployment's `REDIS_CHANNEL`, so deployments
    /// (and test clusters) sharing one Redis keep separate budgets.
    prefix: String,
    script: redis::Script,
    redis: ConnectionManager,
    local_requests: Option<LocalLimiter>,
    local_rows: Option<LocalLimiter>,
    started: Instant,
    /// Milliseconds after `started` until which decisions are made locally.
    fallback_until_ms: AtomicU64,
    metrics: Arc<Metrics>,
}

fn interval_micros(per_period: Option<NonZeroU32>) -> f64 {
    match per_period {
        // u128 → f64 is exact for any realistic period.
        #[allow(
            clippy::cast_precision_loss,
            reason = "microsecond intervals are small"
        )]
        Some(n) => PERIOD.as_micros() as f64 / f64::from(n.get()),
        None => 0.0,
    }
}

impl IngestQuota {
    pub fn new(
        config: IngestQuotaConfig,
        redis: ConnectionManager,
        namespace: &str,
        metrics: Arc<Metrics>,
    ) -> Self {
        let local = |n: Option<NonZeroU32>| n.map(|n| RateLimiter::keyed(Quota::per_minute(n)));
        Self {
            config,
            prefix: format!("{namespace}:ingest-quota:"),
            script: redis::Script::new(SCRIPT),
            redis,
            local_requests: local(config.requests),
            local_rows: local(config.rows),
            started: Instant::now(),
            fallback_until_ms: AtomicU64::new(0),
            metrics,
        }
    }

    pub fn config(&self) -> IngestQuotaConfig {
        self.config
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn in_fallback(&self) -> bool {
        self.elapsed_ms() < self.fallback_until_ms.load(Ordering::Relaxed)
    }

    /// Charges one request of `units` rows to `who`, or refuses it whole.
    pub async fn charge(&self, who: QuotaIdentity, units: u32) -> Result<(), Refusal> {
        if !self.config.is_enabled() {
            return Ok(());
        }
        if !self.in_fallback() {
            match tokio::time::timeout(REDIS_TIMEOUT, self.charge_shared(who, units)).await {
                Ok(Ok(decision)) => return decision,
                Ok(Err(e)) => warn!(
                    error = %e,
                    "Ingest quota: Redis unavailable, enforcing the quota per instance"
                ),
                Err(_) => warn!(
                    timeout_ms = u64::try_from(REDIS_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
                    "Ingest quota: Redis did not answer, enforcing the quota per instance"
                ),
            }
            let hold = u64::try_from(FALLBACK_HOLD.as_millis()).unwrap_or(u64::MAX);
            self.fallback_until_ms
                .store(self.elapsed_ms().saturating_add(hold), Ordering::Relaxed);
        }
        self.metrics.ingest_quota_fallback.inc();
        self.charge_local(who, units)
    }

    async fn charge_shared(
        &self,
        who: QuotaIdentity,
        units: u32,
    ) -> Result<Result<(), Refusal>, redis::RedisError> {
        let mut conn = self.redis.clone();
        let (code, wait_us): (i64, i64) = self
            .script
            .key(format!("{}{}", self.prefix, who.redis_suffix()))
            .arg(interval_micros(self.config.requests))
            .arg(interval_micros(self.config.rows))
            .arg(units)
            .arg(u64::try_from(PERIOD.as_micros()).unwrap_or(u64::MAX))
            .invoke_async(&mut conn)
            .await?;
        let retry_after = Duration::from_micros(u64::try_from(wait_us).unwrap_or(0)).min(PERIOD);
        Ok(match code {
            0 => Ok(()),
            1 => Err(Refusal {
                exceeded: Exceeded::Requests,
                retry_after,
            }),
            _ => Err(Refusal {
                exceeded: Exceeded::Rows,
                retry_after,
            }),
        })
    }

    fn charge_local(&self, who: QuotaIdentity, units: u32) -> Result<(), Refusal> {
        // Rows first, so a request refused for rows has not spent a request;
        // one refused for requests has spent its rows (a conservative error
        // that only exists while Redis is down).
        if let Some(rows) = &self.local_rows
            && let Some(n) = NonZeroU32::new(units)
        {
            match rows.check_key_n(&who, n) {
                Ok(Ok(())) => {}
                Ok(Err(not_until)) => {
                    return Err(Refusal {
                        exceeded: Exceeded::Rows,
                        retry_after: not_until.wait_time_from(rows.clock().now()).min(PERIOD),
                    });
                }
                // More units than the whole budget; configuration validation
                // makes this unreachable for a real batch.
                Err(_) => {
                    return Err(Refusal {
                        exceeded: Exceeded::Rows,
                        retry_after: PERIOD,
                    });
                }
            }
        }
        if let Some(requests) = &self.local_requests
            && let Err(not_until) = requests.check_key(&who)
        {
            return Err(Refusal {
                exceeded: Exceeded::Requests,
                retry_after: not_until.wait_time_from(requests.clock().now()).min(PERIOD),
            });
        }
        Ok(())
    }

    /// Drops refilled entries from the per-instance fallback buckets.
    fn evict_idle(&self) {
        for limiter in [&self.local_requests, &self.local_rows]
            .into_iter()
            .flatten()
        {
            limiter.retain_recent();
            limiter.shrink_to_fit();
        }
    }

    /// Runs [`Self::evict_idle`] every minute until `cancel` fires.
    pub fn spawn_eviction(self: &Arc<Self>, cancel: tokio_util::sync::CancellationToken) {
        let quota = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(PERIOD);
            loop {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    _ = tick.tick() => quota.evict_idle(),
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nz(n: u32) -> Option<NonZeroU32> {
        NonZeroU32::new(n)
    }

    #[test]
    fn config_defaults_overrides_and_validation() {
        let none = IngestQuotaConfig::from_lookup(|_| None).unwrap();
        assert_eq!(none, IngestQuotaConfig::default());
        assert_eq!(none.requests, nz(20_000));
        assert_eq!(none.rows, nz(60_000));

        let set = IngestQuotaConfig::from_lookup(|n| match n {
            "RATE_LIMIT_INGEST_KEY_PER_MINUTE" => Some("0".into()),
            "RATE_LIMIT_INGEST_ROWS_PER_MINUTE" => Some(" 1000 ".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(set.requests, None, "0 disables");
        assert_eq!(set.rows, nz(1000));

        let off = IngestQuotaConfig::from_lookup(|n| {
            n.starts_with("RATE_LIMIT_INGEST_").then(|| "0".to_string())
        })
        .unwrap();
        assert!(!off.is_enabled());

        for bad in ["-1", "lots", "1.5", "499"] {
            assert!(
                IngestQuotaConfig::from_lookup(
                    |n| (n == "RATE_LIMIT_INGEST_ROWS_PER_MINUTE").then(|| bad.to_string())
                )
                .is_err(),
                "{bad}"
            );
        }
        assert!(
            IngestQuotaConfig::from_lookup(
                |n| (n == "RATE_LIMIT_INGEST_ROWS_PER_MINUTE").then(|| "500".to_string())
            )
            .is_ok(),
            "one full batch is the minimum"
        );
    }

    #[test]
    fn units_count_rows_or_kib_whichever_is_larger() {
        assert_eq!(units_for(0, 2), 1, "an empty array still has a body");
        assert_eq!(units_for(500, 60_000), 500, "a full SDK batch costs 500");
        assert_eq!(units_for(1, 1024), 1);
        assert_eq!(units_for(1, 1025), 2);
        assert_eq!(
            units_for(1, 256 * 1024),
            256,
            "one huge row costs its bytes"
        );
        assert!(
            units_for(MAX_BATCH_ROWS, 256 * 1024) <= u32::try_from(MAX_BATCH_ROWS).unwrap(),
            "no admissible request exceeds the minimum row budget"
        );
    }

    #[test]
    fn identities_have_distinct_redis_keys() {
        assert_eq!(QuotaIdentity::SdkKey(7).redis_suffix(), "key:7");
        assert_eq!(QuotaIdentity::User(7).redis_suffix(), "user:7");
        assert_eq!(QuotaIdentity::SdkKey(-1).redis_suffix(), "key:-1");
    }

    // The shared path needs a Redis server; see `server/tests/ingest_quota.rs`
    // for the HTTP-level tests and `shared_quota_*` below for direct ones.

    async fn quota(config: IngestQuotaConfig, namespace: &str) -> Option<IngestQuota> {
        let url = std::env::var("CHECKGATE_TEST_REDIS_URL").ok()?;
        let client = redis::Client::open(url).unwrap();
        let conn = client.get_connection_manager().await.unwrap();
        let namespace = format!("{namespace}-{}", std::process::id());
        Some(IngestQuota::new(config, conn, &namespace, Metrics::new(1)))
    }

    #[tokio::test]
    async fn shared_quota_charges_rows_and_refuses_whole_batches() {
        let config = IngestQuotaConfig {
            requests: nz(100),
            rows: nz(1000),
        };
        let Some(q) = quota(config, "unit-rows").await else {
            return;
        };
        let who = QuotaIdentity::SdkKey(1);
        q.charge(who, 500).await.unwrap();
        q.charge(who, 500).await.unwrap();
        let refused = q.charge(who, 500).await.unwrap_err();
        assert_eq!(refused.exceeded, Exceeded::Rows);
        assert!(
            refused.retry_after > Duration::from_secs(25)
                && refused.retry_after <= Duration::from_secs(31),
            "500 rows refill in 30 s: {:?}",
            refused.retry_after
        );
        // The refused request charged nothing: a small one still fits once
        // the bucket refills a little, and another key is untouched.
        q.charge(QuotaIdentity::SdkKey(2), 1000).await.unwrap();
        q.charge(QuotaIdentity::User(1), 1000).await.unwrap();
        assert_eq!(q.metrics.ingest_quota_fallback.get(), 0);
    }

    #[tokio::test]
    async fn shared_quota_counts_requests() {
        let config = IngestQuotaConfig {
            requests: nz(3),
            rows: None,
        };
        let Some(q) = quota(config, "unit-requests").await else {
            return;
        };
        let who = QuotaIdentity::SdkKey(1);
        for _ in 0..3 {
            q.charge(who, 0).await.unwrap();
        }
        assert_eq!(
            q.charge(who, 0).await.unwrap_err().exceeded,
            Exceeded::Requests
        );
    }

    #[tokio::test]
    async fn shared_quota_is_atomic_under_concurrency() {
        let config = IngestQuotaConfig {
            requests: None,
            rows: nz(5000),
        };
        let Some(q) = quota(config, "unit-concurrent").await else {
            return;
        };
        let q = Arc::new(q);
        let who = QuotaIdentity::SdkKey(9);
        let tasks: Vec<_> = (0..64)
            .map(|_| {
                let q = Arc::clone(&q);
                tokio::spawn(async move { q.charge(who, 500).await.is_ok() })
            })
            .collect();
        let mut accepted = 0;
        for t in tasks {
            if t.await.unwrap() {
                accepted += 1;
            }
        }
        // 5000 rows of burst = 10 batches; refill during the test adds at
        // most one more (83 rows per second).
        assert!(
            (10..=11).contains(&accepted),
            "accepted {accepted} of 64 concurrent 500-row batches"
        );
    }

    #[tokio::test]
    async fn quota_state_expires_once_refilled() {
        let config = IngestQuotaConfig {
            requests: nz(60_000),
            rows: nz(60_000),
        };
        let Some(q) = quota(config, "unit-ttl").await else {
            return;
        };
        let who = QuotaIdentity::SdkKey(3);
        q.charge(who, 500).await.unwrap();
        let mut conn = q.redis.clone();
        let key = format!("{}{}", q.prefix, who.redis_suffix());
        let ttl: i64 = redis::cmd("PTTL")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap();
        // 500 rows at 1 ms each refill in 500 ms; the key lives that long
        // plus a one-second margin, never indefinitely.
        assert!((1000..=1600).contains(&ttl), "ttl {ttl} ms");
    }

    #[tokio::test]
    async fn local_fallback_enforces_the_same_budget() {
        let config = IngestQuotaConfig {
            requests: nz(1000),
            rows: nz(1000),
        };
        // Hold the fallback window open, as after a Redis failure (the
        // failure itself is driven through a proxy in tests/ingest_quota.rs).
        let Some(q) = quota(config, "unit-fallback").await else {
            return;
        };
        q.fallback_until_ms.store(u64::MAX, Ordering::Relaxed);
        let who = QuotaIdentity::SdkKey(4);
        q.charge(who, 500).await.unwrap();
        q.charge(who, 500).await.unwrap();
        assert_eq!(
            q.charge(who, 500).await.unwrap_err().exceeded,
            Exceeded::Rows
        );
        assert_eq!(q.metrics.ingest_quota_fallback.get(), 3);
        q.charge(QuotaIdentity::SdkKey(5), 500).await.unwrap();
    }
}
