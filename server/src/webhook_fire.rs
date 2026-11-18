use crate::state::AppState;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::{PgPool, Row};
use std::time::Duration;
use tracing::{error, info, warn};

type HmacSha256 = Hmac<Sha256>;

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// HMAC-SHA256 of `body` using `secret`. Returns `sha256=<hex>`.
#[allow(
    clippy::expect_used,
    reason = "HMAC accepts a key of any length, so new_from_slice cannot return InvalidLength here"
)]
fn sign_payload(secret: &str, body: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    format!("sha256={}", hex_encode(&mac.finalize().into_bytes()))
}

/// Bytes of a webhook or integration response kept for the delivery log.
pub(crate) const MAX_STORED_RESPONSE_BYTES: usize = 1024;

/// Reads at most [`MAX_STORED_RESPONSE_BYTES`] of a response body, stopping as
/// soon as the cap is reached. Reading the whole body (`text()`) let a
/// receiver stream an arbitrarily large response into memory for the length of
/// the request timeout, and stored all of it where every project viewer can
/// read it.
pub(crate) async fn read_capped_body(mut resp: reqwest::Response) -> String {
    let mut body = Vec::new();
    while body.len() < MAX_STORED_RESPONSE_BYTES {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let room = MAX_STORED_RESPONSE_BYTES.saturating_sub(body.len());
                body.extend_from_slice(chunk.get(..room.min(chunk.len())).unwrap_or_default());
            }
            Ok(None) | Err(_) => break,
        }
    }
    String::from_utf8_lossy(&body).into_owned()
}

/// The shared client for outbound webhook and integration deliveries.
///
/// Redirects are not followed. A receiver could otherwise answer `303` with a
/// `Location` on an internal address (cloud metadata, a service on the
/// server's network); the client would GET it and the response body would be
/// stored in the delivery log, readable by project viewers. A redirect now
/// lands in the log as its 3xx status.
pub(crate) fn delivery_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Checkgate-Webhooks/1.0")
        .build()
        .unwrap_or_default()
}

struct WebhookRow {
    id: String,
    url: String,
    secret: Option<String>,
}

/// Spawn a fire-and-forget task that delivers the event to all enabled webhooks
/// for `env_id`. Called from flag mutation handlers — never blocks the response.
pub fn fire_webhooks(state: AppState, env_id: String, payload: serde_json::Value) {
    tokio::spawn(async move {
        deliver_to_env(&state, &env_id, payload).await;
    });
}

async fn deliver_to_env(state: &AppState, env_id: &str, payload: serde_json::Value) {
    let rows = match sqlx::query(
        "SELECT id::text, url, secret FROM webhooks \
         WHERE environment_id = $1::uuid AND enabled = true",
    )
    .bind(env_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "Failed to query webhooks for delivery");
            return;
        }
    };

    if rows.is_empty() {
        return;
    }

    let body = match serde_json::to_vec(&payload) {
        Ok(b) => b,
        Err(e) => {
            error!(error = %e, "Failed to serialize webhook payload");
            return;
        }
    };

    let hooks: Vec<WebhookRow> = rows
        .iter()
        .map(|r| WebhookRow {
            id: r.get("id"),
            url: r.get("url"),
            secret: r.get("secret"),
        })
        .collect();

    for hook in hooks {
        let sig = hook
            .secret
            .as_deref()
            .map(|s| sign_payload(s, &body))
            .unwrap_or_default();

        deliver_with_retry(state, &hook.id, &hook.url, &sig, &body, &payload).await;
    }
}

async fn deliver_with_retry(
    state: &AppState,
    webhook_id: &str,
    url: &str,
    signature: &str,
    body: &[u8],
    payload: &serde_json::Value,
) {
    let backoff_secs = [1u64, 5, 15];

    for (attempt, &wait) in backoff_secs.iter().enumerate() {
        let mut req = state
            .webhook_client
            .post(url)
            .header("Content-Type", "application/json")
            .header("User-Agent", "Checkgate-Webhooks/1.0")
            .body(body.to_vec());

        if !signature.is_empty() {
            req = req.header("X-Checkgate-Signature", signature);
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16() as i32;
                let body_text = read_capped_body(resp).await;
                let ok = (200..300).contains(&status);

                log_delivery(
                    &state.db,
                    webhook_id,
                    payload,
                    Some(status),
                    Some(&body_text),
                    None,
                )
                .await;

                if ok {
                    info!(webhook_id, status, "Webhook delivered");
                } else {
                    warn!(webhook_id, status, "Webhook returned non-2xx");
                }
                return;
            }
            Err(e) => {
                // The URL may carry a credential (a token in the path or
                // query); keep it out of the stored error and the log.
                let e = e.without_url();
                let is_last = attempt == backoff_secs.len().saturating_sub(1);
                if is_last {
                    let err_str = e.to_string();
                    warn!(webhook_id, error = %e, "Webhook delivery failed after all retries");
                    log_delivery(&state.db, webhook_id, payload, None, None, Some(&err_str)).await;
                } else {
                    warn!(webhook_id, attempt, error = %e, "Webhook delivery failed — retrying");
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                }
            }
        }
    }
}

async fn log_delivery(
    db: &PgPool,
    webhook_id: &str,
    payload: &serde_json::Value,
    status_code: Option<i32>,
    response_body: Option<&str>,
    error: Option<&str>,
) {
    let event = payload
        .get("event")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    if let Err(e) = sqlx::query(
        "INSERT INTO webhook_deliveries (webhook_id, event, payload, status_code, response_body, error) \
         VALUES ($1::uuid, $2, $3, $4, $5, $6)",
    )
    .bind(webhook_id)
    .bind(event)
    .bind(payload)
    .bind(status_code)
    .bind(response_body)
    .bind(error)
    .execute(db)
    .await
    {
        error!(error = %e, "Failed to log webhook delivery");
    }

    // Prune deliveries older than the most recent 200 to keep the table bounded.
    if let Err(e) = sqlx::query(
        "DELETE FROM webhook_deliveries \
         WHERE webhook_id = $1::uuid \
           AND id NOT IN ( \
               SELECT id FROM webhook_deliveries \
               WHERE webhook_id = $1::uuid \
               ORDER BY delivered_at DESC \
               LIMIT 200 \
           )",
    )
    .bind(webhook_id)
    .execute(db)
    .await
    {
        warn!(error = %e, "Failed to prune old webhook deliveries");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Serves each accepted connection one canned HTTP/1.1 response.
    async fn serve(responses: Vec<Vec<u8>>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for response in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let _ = socket.write_all(&response).await;
                let _ = socket.shutdown().await;
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        // The receiver points the client at an internal address. Following it
        // would GET that address and store its body in the delivery log.
        let base = serve(vec![
            b"HTTP/1.1 303 See Other\r\nLocation: http://169.254.169.254/latest/meta-data/\r\nContent-Length: 0\r\n\r\n"
                .to_vec(),
        ])
        .await;
        let resp = delivery_client()
            .post(&base)
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 303);
    }

    #[tokio::test]
    async fn stored_response_bodies_are_capped() {
        let big = "x".repeat(5 * 1024 * 1024);
        let response = format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\n\r\n{big}",
            big.len()
        );
        let base = serve(vec![response.into_bytes()]).await;
        let resp = delivery_client()
            .post(&base)
            .body("{}")
            .send()
            .await
            .unwrap();
        let body = read_capped_body(resp).await;
        assert_eq!(body.len(), MAX_STORED_RESPONSE_BYTES);
    }
}
