use crate::state::AppState;
use sqlx::Row;
use std::time::Duration;
use tracing::{error, info};

/// Claim each due job and apply its flag patch in the same transaction.
pub async fn run(state: AppState) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        execute_due(&state).await;
    }
}

pub(crate) async fn execute_due(state: &AppState) {
    for _ in 0..50 {
        let mut tx = match state.db.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                error!(error = %e, "Scheduler: begin failed");
                return;
            }
        };
        let row = match sqlx::query(
            "SELECT id::text, environment_id::text, flag_key, patch FROM scheduled_changes \
             WHERE executed_at IS NULL AND scheduled_at <= NOW() \
               AND (last_attempt_at IS NULL OR last_attempt_at <= NOW() - INTERVAL '60 seconds') \
             ORDER BY scheduled_at, id LIMIT 1 FOR UPDATE SKIP LOCKED",
        )
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(Some(row)) => row,
            Ok(None) => return,
            Err(e) => {
                error!(error = %e, "Scheduler: claim failed");
                return;
            }
        };
        let id: String = row.get("id");
        let env_id: String = row.get("environment_id");
        let key: String = row.get("flag_key");
        let patch: serde_json::Value = row.get("patch");
        // A savepoint lets us undo a failed flag write while retaining the job claim
        // until its failure/backoff metadata commits. Other workers cannot retry it early.
        if let Err(e) = sqlx::query("SAVEPOINT flag_patch").execute(&mut *tx).await {
            error!(error = %e, id, "Scheduler: savepoint failed");
            return;
        }
        let result = async {
            if crate::api::flags::lock_environment(&mut tx, &env_id).await? {
                return Err(axum::http::StatusCode::CONFLICT);
            }
            let applied =
                crate::api::flags::apply_patch_in_tx(&mut tx, &env_id, &key, patch).await?;
            sqlx::query(
                "UPDATE scheduled_changes SET executed_at = NOW(), last_error = NULL, \
                         last_attempt_at = NOW(), attempts = attempts + 1 WHERE id = $1::uuid",
            )
            .bind(&id)
            .execute(&mut *tx)
            .await
            .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
            Ok(applied)
        }
        .await;
        match result {
            Ok(applied) => {
                if let Err(e) = tx.commit().await {
                    error!(error = %e, id, "Scheduler: commit failed; job remains pending");
                    return;
                }
                crate::api::flags::emit_patch(state, &env_id, &key, applied, None, Some(&id)).await;
                info!(id, key, "Scheduler: change applied");
            }
            Err(status) => {
                // Leave the job pending. Backoff prevents a failed job monopolizing the batch.
                if let Err(e) = sqlx::query("ROLLBACK TO SAVEPOINT flag_patch")
                    .execute(&mut *tx)
                    .await
                {
                    error!(error = %e, id, "Scheduler: could not roll back failed patch");
                    return;
                }
                if let Err(e) = sqlx::query(
                    "UPDATE scheduled_changes SET last_error = $2, last_attempt_at = NOW(), \
                     attempts = attempts + 1 WHERE id = $1::uuid AND executed_at IS NULL",
                )
                .bind(&id)
                .bind(status.to_string())
                .execute(&mut *tx)
                .await
                {
                    error!(error = %e, id, "Scheduler: could not record failure");
                    return;
                }
                if let Err(e) = tx.commit().await {
                    error!(error = %e, id, "Scheduler: could not commit failure status");
                    return;
                }
                error!(id, key, %status, "Scheduler: change failed; retained for retry");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use axum_extra::extract::cookie::Key;
    use checkgate_core::store::FlagStore;
    use dashmap::DashMap;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::{RwLock, broadcast};

    #[tokio::test]
    async fn failed_jobs_retry_and_concurrent_workers_execute_once() {
        let (Ok(db_url), Ok(redis_url)) = (
            std::env::var("CHECKGATE_TEST_DATABASE_URL"),
            std::env::var("CHECKGATE_TEST_REDIS_URL"),
        ) else {
            return;
        };
        let db = sqlx::PgPool::connect(&db_url).await.unwrap();
        sqlx::migrate!().run(&db).await.unwrap();
        let env: String = sqlx::query_scalar(
            "INSERT INTO environments (name,slug,project_id) VALUES ('Scheduler Regression', \
             'scheduler-regression', (SELECT id FROM projects LIMIT 1)) RETURNING id::text",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        let data = json!({"key":"scheduler_retry_regression","is_enabled":true,"rules":[]});
        sqlx::query("INSERT INTO flags (environment_id,key,data) VALUES ($1::uuid,'scheduler_retry_regression',$2)")
            .bind(&env).bind(data).execute(&db).await.unwrap();
        let id: String = sqlx::query_scalar(
            "INSERT INTO scheduled_changes (environment_id,flag_key,scheduled_at,patch) \
             VALUES ($1::uuid,'scheduler_retry_regression',NOW() - INTERVAL '1 second', \
             '{\"is_enabled\":false}') RETURNING id::text",
        )
        .bind(&env)
        .fetch_one(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE FUNCTION scheduler_regression_failure() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN IF NEW.key = 'scheduler_retry_regression' THEN RAISE EXCEPTION 'transient test failure'; \
             END IF; RETURN NEW; END $$",
        ).execute(&db).await.unwrap();
        sqlx::query("CREATE TRIGGER scheduler_regression_failure BEFORE UPDATE ON flags FOR EACH ROW EXECUTE FUNCTION scheduler_regression_failure()")
            .execute(&db).await.unwrap();
        let redis_client = redis::Client::open(redis_url).unwrap();
        let state = AppState {
            db: db.clone(),
            redis_conn: redis_client
                .get_multiplexed_async_connection()
                .await
                .unwrap(),
            redis_client,
            store: Arc::new(FlagStore::new()),
            flag_tx: broadcast::channel(256).0,
            sdk_keys: Arc::new(RwLock::new(vec![])),
            rate_limiter: crate::rate_limit::new_rate_limiter(),
            session_key: Key::generate(),
            connected_clients: Arc::new(DashMap::new()),
            webhook_client: reqwest::Client::new(),
        };
        tokio::time::timeout(Duration::from_secs(5), execute_due(&state))
            .await
            .unwrap();
        let row = sqlx::query(
            "SELECT executed_at, attempts, last_error FROM scheduled_changes WHERE id=$1::uuid",
        )
        .bind(&id)
        .fetch_one(&db)
        .await
        .unwrap();
        assert!(
            row.get::<Option<time::OffsetDateTime>, _>("executed_at")
                .is_none()
        );
        assert_eq!(row.get::<i64, _>("attempts"), 1);
        assert!(row.get::<Option<String>, _>("last_error").is_some());
        let enabled: bool = sqlx::query_scalar("SELECT (data->>'is_enabled')::bool FROM flags WHERE environment_id=$1::uuid AND key='scheduler_retry_regression'")
            .bind(&env).fetch_one(&db).await.unwrap();
        assert!(enabled, "failed patch rolled back");
        sqlx::query("DROP TRIGGER scheduler_regression_failure ON flags")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("DROP FUNCTION scheduler_regression_failure()")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("UPDATE scheduled_changes SET last_attempt_at=NOW() - INTERVAL '2 minutes' WHERE id=$1::uuid")
            .bind(&id).execute(&db).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(execute_due(&state), execute_due(&state));
        })
        .await
        .unwrap();
        let row = sqlx::query(
            "SELECT executed_at, attempts, last_error FROM scheduled_changes WHERE id=$1::uuid",
        )
        .bind(&id)
        .fetch_one(&db)
        .await
        .unwrap();
        assert!(
            row.get::<Option<time::OffsetDateTime>, _>("executed_at")
                .is_some()
        );
        assert_eq!(
            row.get::<i64, _>("attempts"),
            2,
            "one failure and exactly one successful execution"
        );
        assert!(row.get::<Option<String>, _>("last_error").is_none());
        let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM flag_audit_log WHERE environment_id=$1::uuid AND flag_key='scheduler_retry_regression'")
            .bind(&env).fetch_one(&db).await.unwrap();
        assert_eq!(
            audits, 1,
            "concurrent workers do not emit duplicate changes"
        );
        sqlx::query("DELETE FROM environments WHERE id=$1::uuid")
            .bind(&env)
            .execute(&db)
            .await
            .unwrap();
        db.close().await;
    }
}
