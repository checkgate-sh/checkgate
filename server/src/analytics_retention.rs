//! Analytics retention: deletes impressions and goal events older than
//! `ANALYTICS_RETENTION_DAYS`, in small batches, from one instance at a time.
//!
//! **What is deleted.** Rows of `impressions` and `events` whose `received_at`
//! (the server's clock when the row was stored, never the client-supplied
//! time) is older than the retention period. Rows that a running or paused
//! experiment reads are kept: impressions of its flag and events of its goal,
//! in its environment. They expire normally once the experiment is completed
//! or deleted. No other table is touched (audit logs, flags, revisions,
//! deliveries and the rest have their own lifetimes).
//!
//! **How.** Every instance wakes up every `min(interval, 60 s)` and tries a
//! session-level PostgreSQL advisory lock on one pooled connection; the
//! instance that gets it checks the time of the last completed pass (the
//! `analytics_retention_last_run` row of `settings`, in database time) and,
//! when `ANALYTICS_RETENTION_INTERVAL_SECS` has passed, runs a pass:
//!
//! - one cutoff per pass, `NOW() - retention`, so the pass has a fixed end;
//! - batches of `ANALYTICS_RETENTION_BATCH_SIZE` rows walked in `received_at`
//!   order with a keyset cursor (the existing `*_cleanup` indexes), so a
//!   batch never rescans rows an earlier batch kept;
//! - each batch is its own short transaction with `statement_timeout` and
//!   `lock_timeout`, so nothing holds back vacuum or blocks ingestion and
//!   flag writes (which never touch these tables);
//! - a short pause between batches, and a check for shutdown.
//!
//! A failed or interrupted pass is not recorded as completed, so the next
//! check (on any instance) runs it again from the oldest remaining row;
//! everything a finished batch deleted stays deleted, and an unfinished batch
//! rolls back whole. The lock dies with its connection, so a crashed instance
//! never blocks the others. Startup does no deletion: the first check comes
//! one check period after the server starts serving.

use crate::metrics::{AnalyticsTable, AnalyticsTableLabels, Metrics, Outcome, OutcomeLabels};
use sqlx::{PgPool, Row, pool::PoolConnection, postgres::Postgres};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// `pg_try_advisory_lock` key for retention passes ("cgretain" in ASCII).
const LOCK_KEY: i64 = 0x6367_7265_7461_696e;
/// `settings` row holding the Unix time of the last completed pass.
const LAST_RUN_KEY: &str = "analytics_retention_last_run";
/// Longest wait between checks, whatever the interval.
const MAX_CHECK_PERIOD: Duration = Duration::from_secs(60);
/// Pause between batches, so a backlog is deleted at a bounded pace.
const BATCH_PAUSE: Duration = Duration::from_millis(50);
/// Per-batch guards: a batch that takes longer is cancelled (and retried
/// next pass); a lock it cannot get within `LOCK_TIMEOUT` likewise.
const STATEMENT_TIMEOUT: &str = "30s";
const LOCK_TIMEOUT: &str = "5s";

const DEFAULT_DAYS: u32 = 90;
const MAX_DAYS: u32 = 3650;
const DEFAULT_INTERVAL: Duration = Duration::from_secs(3600);
const DEFAULT_BATCH: u32 = 5000;
const MAX_BATCH: u32 = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetentionConfig {
    /// Days analytics rows are kept; `None` keeps them forever.
    pub days: Option<u32>,
    /// Minimum time between completed passes across the deployment.
    pub interval: Duration,
    /// Rows examined per batch.
    pub batch_size: u32,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            days: Some(DEFAULT_DAYS),
            interval: DEFAULT_INTERVAL,
            batch_size: DEFAULT_BATCH,
        }
    }
}

impl RetentionConfig {
    /// Reads `ANALYTICS_RETENTION_DAYS` (`0` keeps rows forever),
    /// `ANALYTICS_RETENTION_INTERVAL_SECS` and `ANALYTICS_RETENTION_BATCH_SIZE`.
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let defaults = Self::default();
        let read = |name: &str, max: u64| -> Result<Option<u64>, String> {
            match lookup(name) {
                None => Ok(None),
                Some(v) => match v.trim().parse::<u64>() {
                    Ok(n) if n <= max => Ok(Some(n)),
                    _ => Err(format!(
                        "{name} must be an integer from 0 to {max}, got {v:?}"
                    )),
                },
            }
        };
        let days = match read("ANALYTICS_RETENTION_DAYS", u64::from(MAX_DAYS))? {
            None => defaults.days,
            Some(0) => None,
            Some(n) => Some(u32::try_from(n).unwrap_or(MAX_DAYS)),
        };
        let interval = match read("ANALYTICS_RETENTION_INTERVAL_SECS", 7 * 86_400)? {
            None => defaults.interval,
            Some(0) => {
                return Err("ANALYTICS_RETENTION_INTERVAL_SECS must be at least 1".into());
            }
            Some(n) => Duration::from_secs(n),
        };
        let batch_size = match read("ANALYTICS_RETENTION_BATCH_SIZE", u64::from(MAX_BATCH))? {
            None => defaults.batch_size,
            Some(n) if n < 100 => {
                return Err(format!(
                    "ANALYTICS_RETENTION_BATCH_SIZE must be from 100 to {MAX_BATCH}, got {n}"
                ));
            }
            Some(n) => u32::try_from(n).unwrap_or(MAX_BATCH),
        };
        Ok(Self {
            days,
            interval,
            batch_size,
        })
    }

    fn check_period(&self) -> Duration {
        self.interval.min(MAX_CHECK_PERIOD)
    }
}

/// One table's retention: the experiment column that protects its rows.
#[derive(Clone, Copy, Debug)]
struct Table {
    label: AnalyticsTable,
    name: &'static str,
    /// Deletes one batch. `$1` cutoff, `$2`/`$3` the cursor (exclusive),
    /// `$4` batch size. Returns rows examined, rows deleted and the new cursor.
    sql: &'static str,
}

/// The batch walks `received_at, id` from the cursor; `received_at >= $2` lets
/// the planner use the `*_cleanup` index, the row comparison breaks ties (one
/// ingest transaction stores all its rows with the same `received_at`).
macro_rules! batch_sql {
    ($table:literal, $key:literal, $experiment_key:literal) => {
        concat!(
            "WITH batch AS ( \
                 SELECT id, received_at, environment_id, ",
            $key,
            " AS k FROM ",
            $table,
            " WHERE received_at < $1 \
                   AND received_at >= $2 \
                   AND (received_at, id) > ($2, $3) \
                 ORDER BY received_at, id \
                 LIMIT $4 \
             ), deleted AS ( \
                 DELETE FROM ",
            $table,
            " t USING batch b \
                 WHERE t.id = b.id \
                   AND NOT EXISTS ( \
                       SELECT 1 FROM experiments x \
                       WHERE x.environment_id = b.environment_id \
                         AND x.",
            $experiment_key,
            " = b.k \
                         AND x.status IN ('running', 'paused')) \
                 RETURNING 1 \
             ), last AS ( \
                 SELECT received_at, id FROM batch ORDER BY received_at DESC, id DESC LIMIT 1 \
             ) \
             SELECT (SELECT count(*) FROM batch) AS examined, \
                    (SELECT count(*) FROM deleted) AS deleted, \
                    (SELECT received_at FROM last) AS last_received_at, \
                    (SELECT id FROM last) AS last_id"
        )
    };
}

const IMPRESSIONS: Table = Table {
    label: AnalyticsTable::impressions,
    name: "impressions",
    sql: batch_sql!("impressions", "flag_key", "flag_key"),
};
const EVENTS: Table = Table {
    label: AnalyticsTable::events,
    name: "events",
    sql: batch_sql!("events", "event_key", "goal_event_key"),
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PassReport {
    pub impressions: u64,
    pub events: u64,
    pub batches: u64,
    /// Shutdown stopped the pass before it finished.
    pub interrupted: bool,
}

/// What a check did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Check {
    /// Another instance holds the lock.
    Busy,
    /// A pass completed within the interval.
    NotDue,
    Ran(PassReport),
}

/// Runs retention checks until `cancel` fires. Returns at once when
/// retention is disabled.
pub async fn run(
    db: PgPool,
    config: RetentionConfig,
    metrics: Arc<Metrics>,
    cancel: CancellationToken,
) {
    let Some(days) = config.days else {
        return;
    };
    let period = config.check_period();
    let mut tick = tokio::time::interval_at(
        tokio::time::Instant::now()
            .checked_add(period)
            .unwrap_or_else(tokio::time::Instant::now),
        period,
    );
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                info!("Analytics retention stopped");
                return;
            }
            _ = tick.tick() => {}
        }
        let started = Instant::now();
        match check_once(&db, days, &config, &cancel).await {
            Ok(Check::Ran(report)) => {
                let elapsed = started.elapsed();
                metrics
                    .analytics_cleanup_duration
                    .observe(elapsed.as_secs_f64());
                for (table, rows) in [
                    (AnalyticsTable::impressions, report.impressions),
                    (AnalyticsTable::events, report.events),
                ] {
                    metrics
                        .analytics_cleanup_deleted
                        .get_or_create(&AnalyticsTableLabels { table })
                        .inc_by(rows);
                }
                if report.interrupted {
                    info!(
                        deleted_impressions = report.impressions,
                        deleted_events = report.events,
                        "Analytics retention pass interrupted by shutdown; the next check resumes it"
                    );
                    return;
                }
                metrics
                    .analytics_cleanup_runs
                    .get_or_create(&OutcomeLabels {
                        outcome: Outcome::success,
                    })
                    .inc();
                metrics.analytics_cleanup_last_success.set(
                    i64::try_from(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0),
                    )
                    .unwrap_or(i64::MAX),
                );
                info!(
                    retention_days = days,
                    deleted_impressions = report.impressions,
                    deleted_events = report.events,
                    batches = report.batches,
                    elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                    "Analytics retention pass complete"
                );
            }
            Ok(Check::Busy | Check::NotDue) => {}
            Err(e) => {
                metrics
                    .analytics_cleanup_runs
                    .get_or_create(&OutcomeLabels {
                        outcome: Outcome::failure,
                    })
                    .inc();
                error!(error = %e, "Analytics retention pass failed; it will be retried");
            }
        }
    }
}

/// Takes the lock, and runs a pass if one is due. The lock is always released
/// before the connection goes back to the pool; if releasing fails, the
/// connection is closed instead, which releases it on the server.
pub(crate) async fn check_once(
    db: &PgPool,
    days: u32,
    config: &RetentionConfig,
    cancel: &CancellationToken,
) -> Result<Check, sqlx::Error> {
    let mut conn = db.acquire().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(LOCK_KEY)
        .fetch_one(&mut *conn)
        .await?;
    if !locked {
        return Ok(Check::Busy);
    }
    let result = locked_check(&mut conn, days, config, cancel).await;
    let released = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
        .bind(LOCK_KEY)
        .fetch_one(&mut *conn)
        .await;
    if !matches!(released, Ok(true)) {
        warn!("Could not release the analytics retention lock; closing its connection");
        drop(conn.detach());
    }
    result
}

async fn locked_check(
    conn: &mut PoolConnection<Postgres>,
    days: u32,
    config: &RetentionConfig,
    cancel: &CancellationToken,
) -> Result<Check, sqlx::Error> {
    let interval_secs = i64::try_from(config.interval.as_secs()).unwrap_or(i64::MAX);
    let due: bool = sqlx::query_scalar(
        "SELECT COALESCE( \
             (SELECT CASE WHEN value ~ '^[0-9]{1,12}$' THEN value::bigint END \
              FROM settings WHERE key = $1) \
             <= extract(epoch FROM NOW())::bigint - $2, \
             true)",
    )
    .bind(LAST_RUN_KEY)
    .bind(interval_secs)
    .fetch_one(&mut **conn)
    .await?;
    if !due {
        return Ok(Check::NotDue);
    }

    let cutoff: time::OffsetDateTime =
        sqlx::query_scalar("SELECT NOW() - make_interval(days => $1)")
            .bind(i32::try_from(days).unwrap_or(i32::MAX))
            .fetch_one(&mut **conn)
            .await?;
    let report = purge(conn, cutoff, config.batch_size, cancel).await?;
    if !report.interrupted {
        sqlx::query(
            "INSERT INTO settings (key, value) \
             VALUES ($1, extract(epoch FROM NOW())::bigint::text) \
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(LAST_RUN_KEY)
        .execute(&mut **conn)
        .await?;
    }
    Ok(Check::Ran(report))
}

/// Deletes every unprotected row stored before `cutoff`, batch by batch.
pub(crate) async fn purge(
    conn: &mut PoolConnection<Postgres>,
    cutoff: time::OffsetDateTime,
    batch_size: u32,
    cancel: &CancellationToken,
) -> Result<PassReport, sqlx::Error> {
    let mut report = PassReport::default();
    for table in [IMPRESSIONS, EVENTS] {
        // The cursor starts at the oldest row (an index lookup), whatever
        // its date; an empty table needs no pass.
        let oldest: Option<time::OffsetDateTime> =
            sqlx::query_scalar(&format!("SELECT min(received_at) FROM {}", table.name))
                .fetch_one(&mut **conn)
                .await?;
        let Some(oldest) = oldest else { continue };
        let mut cursor: (time::OffsetDateTime, i64) = (oldest, 0);
        loop {
            if cancel.is_cancelled() {
                report.interrupted = true;
                return Ok(report);
            }
            let batch = delete_batch(conn, table, cutoff, cursor, batch_size).await?;
            report.batches = report.batches.saturating_add(1);
            match table.label {
                AnalyticsTable::impressions => {
                    report.impressions = report.impressions.saturating_add(batch.deleted);
                }
                AnalyticsTable::events => {
                    report.events = report.events.saturating_add(batch.deleted);
                }
            }
            match batch.last {
                Some(last) if batch.examined >= u64::from(batch_size) => cursor = last,
                _ => break,
            }
            tokio::select! {
                () = cancel.cancelled() => {
                    report.interrupted = true;
                    return Ok(report);
                }
                () = tokio::time::sleep(BATCH_PAUSE) => {}
            }
        }
    }
    Ok(report)
}

struct Batch {
    examined: u64,
    deleted: u64,
    last: Option<(time::OffsetDateTime, i64)>,
}

async fn delete_batch(
    conn: &mut PoolConnection<Postgres>,
    table: Table,
    cutoff: time::OffsetDateTime,
    cursor: (time::OffsetDateTime, i64),
    batch_size: u32,
) -> Result<Batch, sqlx::Error> {
    let mut tx = sqlx::Connection::begin(&mut **conn).await?;
    sqlx::query(&format!(
        "SET LOCAL statement_timeout = '{STATEMENT_TIMEOUT}'"
    ))
    .execute(&mut *tx)
    .await?;
    sqlx::query(&format!("SET LOCAL lock_timeout = '{LOCK_TIMEOUT}'"))
        .execute(&mut *tx)
        .await?;
    let row = sqlx::query(table.sql)
        .bind(cutoff)
        .bind(cursor.0)
        .bind(cursor.1)
        .bind(i64::from(batch_size))
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    let examined: i64 = row.get("examined");
    let deleted: i64 = row.get("deleted");
    let last_at: Option<time::OffsetDateTime> = row.get("last_received_at");
    let last_id: Option<i64> = row.get("last_id");
    Ok(Batch {
        examined: u64::try_from(examined).unwrap_or(0),
        deleted: u64::try_from(deleted).unwrap_or(0),
        last: last_at.zip(last_id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn config_defaults() {
        let c = RetentionConfig::from_lookup(lookup(&[])).unwrap();
        assert_eq!(c.days, Some(90));
        assert_eq!(c.interval, Duration::from_secs(3600));
        assert_eq!(c.batch_size, 5000);
        assert_eq!(c.check_period(), Duration::from_secs(60));
    }

    #[test]
    fn config_overrides_and_disable() {
        let c = RetentionConfig::from_lookup(lookup(&[
            ("ANALYTICS_RETENTION_DAYS", " 30 "),
            ("ANALYTICS_RETENTION_INTERVAL_SECS", "5"),
            ("ANALYTICS_RETENTION_BATCH_SIZE", "100"),
        ]))
        .unwrap();
        assert_eq!(c.days, Some(30));
        assert_eq!(c.interval, Duration::from_secs(5));
        assert_eq!(c.check_period(), Duration::from_secs(5));
        assert_eq!(c.batch_size, 100);
        let off =
            RetentionConfig::from_lookup(lookup(&[("ANALYTICS_RETENTION_DAYS", "0")])).unwrap();
        assert_eq!(off.days, None, "0 keeps rows forever");
    }

    #[test]
    fn config_rejects_invalid_values() {
        for (name, value) in [
            ("ANALYTICS_RETENTION_DAYS", "-1"),
            ("ANALYTICS_RETENTION_DAYS", "3651"),
            ("ANALYTICS_RETENTION_DAYS", "30d"),
            ("ANALYTICS_RETENTION_INTERVAL_SECS", "0"),
            ("ANALYTICS_RETENTION_INTERVAL_SECS", "604801"),
            ("ANALYTICS_RETENTION_BATCH_SIZE", "99"),
            ("ANALYTICS_RETENTION_BATCH_SIZE", "100001"),
            ("ANALYTICS_RETENTION_BATCH_SIZE", ""),
        ] {
            assert!(
                RetentionConfig::from_lookup(lookup(&[(name, value)])).is_err(),
                "{name}={value:?}"
            );
        }
    }

    #[test]
    fn batch_sql_names_the_right_columns() {
        assert!(IMPRESSIONS.sql.contains("flag_key AS k FROM impressions"));
        assert!(IMPRESSIONS.sql.contains("x.flag_key = b.k"));
        assert!(EVENTS.sql.contains("event_key AS k FROM events"));
        assert!(EVENTS.sql.contains("x.goal_event_key = b.k"));
    }

    // ── Against PostgreSQL ──────────────────────────────────────────────────
    //
    // Each test gets its own database: retention deletes from whole tables.

    async fn fresh_db(name: &str) -> Option<PgPool> {
        let admin_url = std::env::var("CHECKGATE_TEST_DATABASE_URL").ok()?;
        let name = format!("cg_unit_{name}_{}", std::process::id());
        let admin = PgPool::connect(&admin_url).await.unwrap();
        sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query(&format!("CREATE DATABASE \"{name}\""))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        let mut url = reqwest::Url::parse(&admin_url).unwrap();
        url.set_path(&format!("/{name}"));
        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(url.as_str())
            .await
            .unwrap();
        sqlx::migrate!().run(&db).await.unwrap();
        Some(db)
    }

    async fn env(db: &PgPool, slug: &str) -> String {
        sqlx::query_scalar(
            "INSERT INTO environments (name, slug, project_id) \
             VALUES ($1, $1, (SELECT id FROM projects LIMIT 1)) RETURNING id::text",
        )
        .bind(slug)
        .fetch_one(db)
        .await
        .unwrap()
    }

    /// Inserts `n` impressions of `flag` received at `at` (one statement, so
    /// they share a timestamp like one ingest transaction's rows do).
    async fn impressions_at(db: &PgPool, env: &str, flag: &str, at: time::OffsetDateTime, n: i64) {
        sqlx::query(
            "INSERT INTO impressions (environment_id, flag_key, user_id, value, received_at) \
             SELECT $1::uuid, $2, 'u' || g, 'true', $3 FROM generate_series(1, $4) g",
        )
        .bind(env)
        .bind(flag)
        .bind(at)
        .bind(n)
        .execute(db)
        .await
        .unwrap();
    }

    async fn events_at(db: &PgPool, env: &str, key: &str, at: time::OffsetDateTime, n: i64) {
        sqlx::query(
            "INSERT INTO events (environment_id, event_key, user_id, received_at) \
             SELECT $1::uuid, $2, 'u' || g, $3 FROM generate_series(1, $4) g",
        )
        .bind(env)
        .bind(key)
        .bind(at)
        .bind(n)
        .execute(db)
        .await
        .unwrap();
    }

    async fn count(db: &PgPool, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(db).await.unwrap()
    }

    async fn now(db: &PgPool) -> time::OffsetDateTime {
        sqlx::query_scalar("SELECT NOW()")
            .fetch_one(db)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn cutoff_boundary_is_exclusive() {
        let Some(db) = fresh_db("retention_boundary").await else {
            return;
        };
        let e = env(&db, "boundary").await;
        let cutoff = now(&db).await - time::Duration::days(90);
        let us = time::Duration::microseconds(1);
        impressions_at(&db, &e, "before", cutoff - us, 3).await;
        impressions_at(&db, &e, "at", cutoff, 3).await;
        impressions_at(&db, &e, "after", cutoff + us, 3).await;
        impressions_at(&db, &e, "ancient", cutoff - time::Duration::days(3000), 2).await;
        events_at(&db, &e, "before", cutoff - us, 2).await;
        events_at(&db, &e, "at", cutoff, 2).await;

        let mut conn = db.acquire().await.unwrap();
        let report = purge(&mut conn, cutoff, 100, &CancellationToken::new())
            .await
            .unwrap();
        drop(conn);
        assert_eq!(report.impressions, 5, "{report:?}");
        assert_eq!(report.events, 2, "{report:?}");
        assert!(!report.interrupted);
        assert_eq!(
            count(
                &db,
                "SELECT count(*) FROM impressions WHERE flag_key IN ('before','ancient')"
            )
            .await,
            0
        );
        assert_eq!(
            count(
                &db,
                "SELECT count(*) FROM impressions WHERE flag_key IN ('at','after')"
            )
            .await,
            6,
            "a row stored exactly at the cutoff is kept"
        );
        assert_eq!(
            count(&db, "SELECT count(*) FROM events WHERE event_key = 'at'").await,
            2
        );
        db.close().await;
    }

    #[tokio::test]
    async fn rows_of_running_and_paused_experiments_are_kept() {
        let Some(db) = fresh_db("retention_experiments").await else {
            return;
        };
        let a = env(&db, "exp-a").await;
        let b = env(&db, "exp-b").await;
        for (key, status) in [
            ("run", "running"),
            ("pause", "paused"),
            ("done", "completed"),
        ] {
            sqlx::query(
                "INSERT INTO experiments (environment_id, key, name, flag_key, goal_event_key, status) \
                 VALUES ($1::uuid, $2, $2, $3, $4, $5)",
            )
            .bind(&a)
            .bind(key)
            .bind(format!("flag-{key}"))
            .bind(format!("goal-{key}"))
            .bind(status)
            .execute(&db)
            .await
            .unwrap();
        }
        let old = now(&db).await - time::Duration::days(200);
        for key in ["run", "pause", "done"] {
            impressions_at(&db, &a, &format!("flag-{key}"), old, 4).await;
            events_at(&db, &a, &format!("goal-{key}"), old, 4).await;
            // Same keys in another environment are not protected.
            impressions_at(&db, &b, &format!("flag-{key}"), old, 4).await;
            events_at(&db, &b, &format!("goal-{key}"), old, 4).await;
        }
        // An event named like a running experiment's *flag* is not its goal.
        events_at(&db, &a, "flag-run", old, 4).await;

        let mut conn = db.acquire().await.unwrap();
        let cutoff = now(&db).await - time::Duration::days(90);
        let report = purge(&mut conn, cutoff, 100, &CancellationToken::new())
            .await
            .unwrap();
        drop(conn);
        assert_eq!(report.impressions, 4 + 12, "{report:?}");
        assert_eq!(report.events, 4 + 12 + 4, "{report:?}");
        let kept_imp = count(
            &db,
            "SELECT count(*) FROM impressions i JOIN environments e ON e.id = i.environment_id \
             WHERE e.slug = 'exp-a' AND flag_key IN ('flag-run','flag-pause')",
        )
        .await;
        let kept_ev = count(
            &db,
            "SELECT count(*) FROM events v JOIN environments e ON e.id = v.environment_id \
             WHERE e.slug = 'exp-a' AND event_key IN ('goal-run','goal-pause')",
        )
        .await;
        assert_eq!((kept_imp, kept_ev), (8, 8));
        assert_eq!(count(&db, "SELECT count(*) FROM impressions").await, 8);
        assert_eq!(count(&db, "SELECT count(*) FROM events").await, 8);

        // Completing the experiment releases its rows on the next pass.
        sqlx::query("UPDATE experiments SET status = 'completed' WHERE key = 'run'")
            .execute(&db)
            .await
            .unwrap();
        let mut conn = db.acquire().await.unwrap();
        purge(&mut conn, cutoff, 100, &CancellationToken::new())
            .await
            .unwrap();
        drop(conn);
        assert_eq!(count(&db, "SELECT count(*) FROM impressions").await, 4);
        db.close().await;
    }

    #[tokio::test]
    async fn batches_are_bounded_and_walk_past_kept_rows_and_ties() {
        let Some(db) = fresh_db("retention_batches").await else {
            return;
        };
        let e = env(&db, "batches").await;
        sqlx::query(
            "INSERT INTO experiments (environment_id, key, name, flag_key, goal_event_key) \
             VALUES ($1::uuid, 'x', 'x', 'kept', 'g')",
        )
        .bind(&e)
        .execute(&db)
        .await
        .unwrap();
        let t = now(&db).await - time::Duration::days(100);
        // 1,050 expired rows in three timestamp groups (ties across batch
        // boundaries), with 300 protected rows interleaved by timestamp.
        impressions_at(&db, &e, "gone", t, 350).await;
        impressions_at(&db, &e, "kept", t, 150).await;
        impressions_at(&db, &e, "gone", t + time::Duration::seconds(1), 350).await;
        impressions_at(&db, &e, "kept", t + time::Duration::seconds(2), 150).await;
        impressions_at(&db, &e, "gone", t + time::Duration::seconds(3), 350).await;
        impressions_at(&db, &e, "fresh", now(&db).await, 10).await;

        let mut conn = db.acquire().await.unwrap();
        let cutoff = now(&db).await - time::Duration::days(90);
        let report = purge(&mut conn, cutoff, 100, &CancellationToken::new())
            .await
            .unwrap();
        drop(conn);
        assert_eq!(report.impressions, 1050);
        // 1,350 expired rows examined 100 at a time: 14 batches (the empty
        // events table needs none). Kept rows are examined once each.
        assert_eq!(report.batches, 14, "{report:?}");
        assert_eq!(
            count(
                &db,
                "SELECT count(*) FROM impressions WHERE flag_key = 'gone'"
            )
            .await,
            0
        );
        assert_eq!(
            count(
                &db,
                "SELECT count(*) FROM impressions WHERE flag_key = 'kept'"
            )
            .await,
            300
        );
        assert_eq!(
            count(
                &db,
                "SELECT count(*) FROM impressions WHERE flag_key = 'fresh'"
            )
            .await,
            10
        );
        db.close().await;
    }

    fn config(interval_secs: u64) -> RetentionConfig {
        RetentionConfig {
            days: Some(90),
            interval: Duration::from_secs(interval_secs),
            batch_size: 100,
        }
    }

    #[tokio::test]
    async fn one_pass_at_a_time_and_at_most_one_per_interval() {
        let Some(db) = fresh_db("retention_lock").await else {
            return;
        };
        let e = env(&db, "lock").await;
        impressions_at(
            &db,
            &e,
            "old",
            now(&db).await - time::Duration::days(91),
            50,
        )
        .await;
        let cancel = CancellationToken::new();

        // A held lock (another instance mid-pass) means Busy, and nothing is deleted.
        let mut holder = db.acquire().await.unwrap();
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(LOCK_KEY)
            .execute(&mut *holder)
            .await
            .unwrap();
        assert_eq!(
            check_once(&db, 90, &config(3600), &cancel).await.unwrap(),
            Check::Busy
        );
        assert_eq!(count(&db, "SELECT count(*) FROM impressions").await, 50);
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(LOCK_KEY)
            .execute(&mut *holder)
            .await
            .unwrap();
        drop(holder);

        // Four concurrent checks (four instances): exactly one runs the pass.
        let hourly = config(3600);
        let results =
            futures_util::future::join_all((0..4).map(|_| check_once(&db, 90, &hourly, &cancel)))
                .await;
        let ran: Vec<_> = results
            .iter()
            .filter_map(|r| match r {
                Ok(Check::Ran(report)) => Some(report.impressions),
                _ => None,
            })
            .collect();
        assert_eq!(ran, vec![50], "{results:?}");
        assert!(
            results
                .iter()
                .all(|r| matches!(r, Ok(Check::Ran(_) | Check::Busy | Check::NotDue)))
        );
        // Within the interval, no instance runs another pass.
        assert_eq!(
            check_once(&db, 90, &config(3600), &cancel).await.unwrap(),
            Check::NotDue
        );
        // Once it has passed, the next check does.
        sqlx::query(
            "UPDATE settings SET value = (extract(epoch FROM NOW())::bigint - 3601)::text WHERE key = $1",
        )
        .bind(LAST_RUN_KEY)
        .execute(&db)
        .await
        .unwrap();
        assert!(matches!(
            check_once(&db, 90, &config(3600), &cancel).await.unwrap(),
            Check::Ran(_)
        ));
        // The lock is released after every check: a plain session can take it.
        let mut probe = db.acquire().await.unwrap();
        let free: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(LOCK_KEY)
            .fetch_one(&mut *probe)
            .await
            .unwrap();
        assert!(free);
        drop(probe.detach());
        db.close().await;
    }

    #[tokio::test]
    async fn failed_batch_rolls_back_whole_and_the_next_check_resumes() {
        let Some(db) = fresh_db("retention_failure").await else {
            return;
        };
        let e = env(&db, "failure").await;
        let old = now(&db).await - time::Duration::days(120);
        impressions_at(&db, &e, "a", old, 250).await;
        impressions_at(&db, &e, "poison", old + time::Duration::seconds(1), 1).await;
        impressions_at(&db, &e, "b", old + time::Duration::seconds(2), 250).await;
        // A failure inside the third batch (rows 201–300 in received_at, id order).
        sqlx::query(
            "CREATE FUNCTION retention_test_fail() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN IF OLD.flag_key = 'poison' THEN RAISE EXCEPTION 'injected failure'; END IF; \
             RETURN OLD; END $$",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER retention_test_fail BEFORE DELETE ON impressions \
             FOR EACH ROW EXECUTE FUNCTION retention_test_fail()",
        )
        .execute(&db)
        .await
        .unwrap();
        let cancel = CancellationToken::new();
        let err = check_once(&db, 90, &config(3600), &cancel).await;
        assert!(err.is_err(), "{err:?}");
        // Two finished batches stay deleted; the failed one rolled back whole.
        assert_eq!(count(&db, "SELECT count(*) FROM impressions").await, 301);
        assert_eq!(
            count(&db, "SELECT count(*) FROM impressions WHERE flag_key = 'a'").await,
            50
        );
        // Not recorded as a completed pass, and the lock was released.
        assert_eq!(
            count(
                &db,
                "SELECT count(*) FROM settings WHERE key = 'analytics_retention_last_run'"
            )
            .await,
            0
        );
        sqlx::query("DROP TRIGGER retention_test_fail ON impressions")
            .execute(&db)
            .await
            .unwrap();
        let next = check_once(&db, 90, &config(3600), &cancel).await.unwrap();
        assert!(
            matches!(next, Check::Ran(r) if r.impressions == 301),
            "{next:?}"
        );
        assert_eq!(count(&db, "SELECT count(*) FROM impressions").await, 0);
        db.close().await;
    }

    #[tokio::test]
    async fn a_lost_connection_releases_the_lock() {
        let Some(db) = fresh_db("retention_conn_lost").await else {
            return;
        };
        // An instance holding the lock dies (its backend is terminated).
        let mut holder = db.acquire().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *holder)
            .await
            .unwrap();
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(LOCK_KEY)
            .execute(&mut *holder)
            .await
            .unwrap();
        let cancel = CancellationToken::new();
        assert_eq!(
            check_once(&db, 90, &config(3600), &cancel).await.unwrap(),
            Check::Busy
        );
        sqlx::query("SELECT pg_terminate_backend($1)")
            .bind(pid)
            .execute(&db)
            .await
            .unwrap();
        drop(holder.detach());
        let mut ran = false;
        for _ in 0..50 {
            if matches!(
                check_once(&db, 90, &config(3600), &cancel).await,
                Ok(Check::Ran(_))
            ) {
                ran = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(ran, "another instance takes over once the holder is gone");
        db.close().await;
    }

    #[tokio::test]
    async fn shutdown_interrupts_a_pass_between_batches() {
        let Some(db) = fresh_db("retention_cancel").await else {
            return;
        };
        let e = env(&db, "cancel").await;
        impressions_at(
            &db,
            &e,
            "old",
            now(&db).await - time::Duration::days(91),
            1000,
        )
        .await;
        let cancel = CancellationToken::new();
        let pass = {
            let db = db.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move { check_once(&db, 90, &config(3600), &cancel).await })
        };
        // Batches of 100 with a 50 ms pause: cancel mid-pass.
        tokio::time::sleep(Duration::from_millis(120)).await;
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(5), pass)
            .await
            .expect("pass stops promptly")
            .unwrap()
            .unwrap();
        let Check::Ran(report) = result else {
            panic!("{result:?}")
        };
        assert!(report.interrupted);
        let left = count(&db, "SELECT count(*) FROM impressions").await;
        assert!(
            left > 0 && left < 1000 && left % 100 == 0,
            "{left} left: whole batches only"
        );
        assert_eq!(
            count(
                &db,
                "SELECT count(*) FROM settings WHERE key = 'analytics_retention_last_run'"
            )
            .await,
            0,
            "an interrupted pass is not recorded, so the next check resumes it"
        );
        db.close().await;
    }
}
