//! Admission of an analytics ingest request (impressions or goal events),
//! shared by both endpoints. Everything here runs before any database write:
//!
//! 1. the caller may ingest into this environment (`check_ingest_access`);
//! 2. the body — already capped at 256 KiB by the router's body limit — is a
//!    JSON array (`415`/`400`/`422`, as axum's `Json` extractor answered);
//! 3. the batch has at most [`MAX_BATCH_ROWS`] rows (`413`);
//! 4. the caller's credential has quota left for it (`429`, nothing stored).
//!
//! The body is read as bytes so the quota can charge large bodies by size.

use crate::auth::{AuthContext, get_session_claims, sdk_key};
use crate::ingest_quota::{Exceeded, MAX_BATCH_ROWS, QuotaIdentity, units_for};
use crate::metrics::{AnalyticsTable, QuotaLimit, QuotaLimitLabels, RowOutcome};
use crate::state::AppState;
use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::de::DeserializeOwned;
use std::time::Duration;
use tracing::warn;

/// Why an ingest request was not stored; rendered as the response.
#[derive(Debug)]
pub(super) enum Refused {
    Status(StatusCode),
    /// A body the JSON parser rejected, with its message (as axum's `Json`).
    Body(StatusCode, String),
    /// The ingest quota: `429` with `Retry-After`.
    Quota(Duration),
}

impl From<StatusCode> for Refused {
    fn from(status: StatusCode) -> Self {
        Refused::Status(status)
    }
}

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        match self {
            Refused::Status(status) => status.into_response(),
            Refused::Body(status, message) => (status, message).into_response(),
            Refused::Quota(wait) => crate::rate_limit::too_many_requests(wait),
        }
    }
}

/// The credential the request is charged to. `require_auth` admits exactly
/// one of a session, a personal access token or an SDK key.
fn identity(jar: &AuthContext) -> Option<QuotaIdentity> {
    if let Some(claims) = get_session_claims(jar) {
        return Some(QuotaIdentity::User(claims.user_id));
    }
    sdk_key(jar).map(|key| QuotaIdentity::SdkKey(key.id))
}

/// `application/json` or any `application/*+json`, as axum's `Json` accepts.
fn is_json(headers: &HeaderMap) -> bool {
    let Some(value) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let essence = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match essence.split_once('/') {
        Some(("application", subtype)) => subtype == "json" || subtype.ends_with("+json"),
        _ => false,
    }
}

fn parse<T: DeserializeOwned>(headers: &HeaderMap, body: &[u8]) -> Result<Vec<T>, Refused> {
    if !is_json(headers) {
        return Err(Refused::Body(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Expected request with `Content-Type: application/json`".into(),
        ));
    }
    serde_json::from_slice(body).map_err(|e| {
        let status = match e.classify() {
            serde_json::error::Category::Data => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::BAD_REQUEST,
        };
        Refused::Body(
            status,
            format!("Failed to parse the request body as JSON: {e}"),
        )
    })
}

/// Runs the admission steps above and returns the parsed batch (possibly
/// empty) for the caller to store.
pub(super) async fn admit<T: DeserializeOwned>(
    state: &AppState,
    jar: &AuthContext,
    env_id: &str,
    headers: &HeaderMap,
    body: &Bytes,
    table: AnalyticsTable,
) -> Result<Vec<T>, Refused> {
    super::flags::check_ingest_access(&state.db, jar, env_id).await?;

    let batch: Vec<T> = parse(headers, body)?;

    if batch.len() > MAX_BATCH_ROWS {
        warn!(
            ?table,
            count = batch.len(),
            max = MAX_BATCH_ROWS,
            "Analytics batch too large — rejected"
        );
        state
            .metrics
            .count_rows(table, RowOutcome::batch_too_large, batch.len());
        return Err(Refused::Status(StatusCode::PAYLOAD_TOO_LARGE));
    }

    if let Some(who) = identity(jar) {
        let units = units_for(batch.len(), body.len());
        if let Err(refusal) = state.ingest_quota.charge(who, units).await {
            let limit = match refusal.exceeded {
                Exceeded::Requests => QuotaLimit::requests,
                Exceeded::Rows => QuotaLimit::rows,
            };
            state
                .metrics
                .ingest_quota_rejections
                .get_or_create(&QuotaLimitLabels { limit })
                .inc();
            state
                .metrics
                .count_rows(table, RowOutcome::quota, batch.len());
            warn!(
                ?table,
                credential = ?who,
                ?limit,
                rows = batch.len(),
                units,
                retry_after_ms = u64::try_from(refusal.retry_after.as_millis()).unwrap_or(u64::MAX),
                "Ingest quota exceeded — batch refused"
            );
            return Err(Refused::Quota(refusal.retry_after));
        }
    }

    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn status<T>(r: Result<Vec<T>, Refused>) -> StatusCode {
        match r {
            Err(refused) => refused.into_response().status(),
            Ok(_) => StatusCode::OK,
        }
    }

    fn with_type(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn json_content_types() {
        for ok in [
            "application/json",
            "application/json; charset=utf-8",
            "Application/JSON",
            "application/vnd.api+json",
        ] {
            assert!(is_json(&with_type(ok)), "{ok}");
        }
        for bad in ["text/plain", "application/xml", "json", ""] {
            assert!(!is_json(&with_type(bad)), "{bad}");
        }
        assert!(!is_json(&HeaderMap::new()));
    }

    #[test]
    fn parse_errors_keep_the_extractor_status_codes() {
        #[derive(serde::Deserialize)]
        struct Row {
            #[allow(dead_code)]
            key: String,
        }
        let json = with_type("application/json");
        assert_eq!(
            status(parse::<Row>(&HeaderMap::new(), b"[]")),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        assert_eq!(status(parse::<Row>(&json, b"[{")), StatusCode::BAD_REQUEST);
        assert_eq!(
            status(parse::<Row>(&json, br#"[{"nope":1}]"#)),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(parse::<Row>(&json, br#"[{"key":"a"}]"#).unwrap().len(), 1);
    }
}
