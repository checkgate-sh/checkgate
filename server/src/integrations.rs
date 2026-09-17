//! Chat integrations — render Checkgate events as native Slack / Microsoft
//! Teams messages and post them to a configured incoming-webhook URL.
//!
//! Delivery mirrors [`crate::webhook_fire`]: fire-and-forget, retried with
//! backoff, and logged to a bounded per-integration table. The difference is
//! the payload — instead of our own JSON envelope, each provider gets the
//! message shape it renders natively (Slack Block Kit, Teams Adaptive Card).

use crate::state::AppState;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::time::Duration;
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// Event vocabulary
// ---------------------------------------------------------------------------

/// Every event an integration can subscribe to. Exposed to the dashboard so the
/// filter UI never drifts from what the server actually emits.
pub const SUPPORTED_EVENTS: &[&str] = &[
    "flag.created",
    "flag.updated",
    "flag.deleted",
    "flag.promoted",
    "change_request.opened",
    "change_request.approved",
    "change_request.rejected",
];

/// Does this integration want `event`? An empty subscription list means "all",
/// so a freshly-created integration is useful without configuring anything.
fn wants_event(subscribed: &[String], event: &str) -> bool {
    subscribed.is_empty() || subscribed.iter().any(|e| e == event)
}

// ---------------------------------------------------------------------------
// Presentation helpers
// ---------------------------------------------------------------------------

/// Human-readable title for an event, e.g. `flag.created` → "Flag created".
fn event_title(event: &str) -> &str {
    match event {
        "flag.created" => "Flag created",
        "flag.updated" => "Flag updated",
        "flag.deleted" => "Flag deleted",
        "flag.promoted" => "Flag promoted",
        "change_request.opened" => "Change request opened",
        "change_request.approved" => "Change request approved",
        "change_request.rejected" => "Change request rejected",
        other => other,
    }
}

/// Accent colour per event class. Slack takes a hex attachment colour; Teams
/// takes the same value as the card's `themeColor`.
fn event_color(event: &str) -> &'static str {
    match event {
        "flag.deleted" | "change_request.rejected" => "D64545", // red
        "change_request.opened" => "E8A33D",                    // amber — needs a human
        "change_request.approved" => "3DA35D",                  // green
        _ => "10B981",                                          // emerald (brand default)
    }
}

/// `true` when the flag payload says the flag is on. Used to show an at-a-glance
/// enabled/disabled state rather than making people read raw JSON.
fn flag_enabled(payload: &Value) -> Option<bool> {
    payload
        .get("flag")?
        .get("is_enabled")
        .and_then(Value::as_bool)
}

/// The `key: value` context lines shared by both providers, in display order.
/// Returned as pairs so each formatter can lay them out in its own idiom.
fn detail_fields(payload: &Value, env_name: &str) -> Vec<(String, String)> {
    let mut fields = Vec::new();

    fields.push(("Environment".to_string(), env_name.to_string()));

    if let Some(key) = payload.get("flag_key").and_then(Value::as_str) {
        fields.push(("Flag".to_string(), key.to_string()));
    }
    if let Some(enabled) = flag_enabled(payload) {
        let state = if enabled { "Enabled" } else { "Disabled" };
        fields.push(("State".to_string(), state.to_string()));
    }
    if let Some(rollout) = payload
        .get("flag")
        .and_then(|f| f.get("rollout_percentage"))
        .and_then(Value::as_i64)
    {
        fields.push(("Rollout".to_string(), format!("{rollout}%")));
    }
    if let Some(actor) = payload.get("actor_email").and_then(Value::as_str) {
        fields.push(("By".to_string(), actor.to_string()));
    }
    if let Some(reason) = payload
        .get("metadata")
        .and_then(|m| m.get("reason"))
        .and_then(Value::as_str)
        && !reason.is_empty()
    {
        fields.push(("Reason".to_string(), reason.to_string()));
    }

    fields
}

// ---------------------------------------------------------------------------
// Slack — Block Kit
// ---------------------------------------------------------------------------

/// Slack renders `text` as the notification/fallback line and `blocks` as the
/// message body. Both are set so the message is readable in a push notification
/// and in the channel.
pub fn format_slack(payload: &Value, env_name: &str) -> Value {
    let event = payload.get("event").and_then(Value::as_str).unwrap_or("");
    let title = event_title(event);
    let fields = detail_fields(payload, env_name);

    let fallback = match payload.get("flag_key").and_then(Value::as_str) {
        Some(key) => format!("{title}: {key} ({env_name})"),
        None => format!("{title} ({env_name})"),
    };

    // Block Kit caps a section at 10 fields; our detail list is well under
    // that, but chunking keeps it correct if the list grows.
    let field_blocks: Vec<Value> = fields
        .iter()
        .take(10)
        .map(|(k, v)| json!({ "type": "mrkdwn", "text": format!("*{k}*\n{v}") }))
        .collect();

    let mut blocks = vec![json!({
        "type": "header",
        "text": { "type": "plain_text", "text": title, "emoji": true }
    })];

    if !field_blocks.is_empty() {
        blocks.push(json!({ "type": "section", "fields": field_blocks }));
    }

    json!({
        "text": fallback,
        "attachments": [{
            "color": format!("#{}", event_color(event)),
            "blocks": blocks,
        }],
    })
}

// ---------------------------------------------------------------------------
// Microsoft Teams — MessageCard
// ---------------------------------------------------------------------------

/// Teams incoming webhooks accept the legacy `MessageCard` schema, which is what
/// a plain (non-Bot-Framework) connector renders. Adaptive Cards require a bot
/// registration, which this alerts-only integration deliberately avoids.
pub fn format_teams(payload: &Value, env_name: &str) -> Value {
    let event = payload.get("event").and_then(Value::as_str).unwrap_or("");
    let title = event_title(event);
    let fields = detail_fields(payload, env_name);

    let facts: Vec<Value> = fields
        .iter()
        .map(|(k, v)| json!({ "name": k, "value": v }))
        .collect();

    let summary = match payload.get("flag_key").and_then(Value::as_str) {
        Some(key) => format!("{title}: {key}"),
        None => title.to_string(),
    };

    json!({
        "@type": "MessageCard",
        "@context": "https://schema.org/extensions",
        "themeColor": event_color(event),
        "summary": summary,
        "sections": [{
            "activityTitle": title,
            "facts": facts,
            "markdown": false,
        }],
    })
}

/// Render `payload` for `kind`. Returns `None` for an unknown provider rather
/// than guessing — the DB constrains `kind`, so this is defence in depth.
pub fn format_for(kind: &str, payload: &Value, env_name: &str) -> Option<Value> {
    match kind {
        "slack" => Some(format_slack(payload, env_name)),
        "teams" => Some(format_teams(payload, env_name)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------

struct IntegrationRow {
    id: String,
    kind: String,
    webhook_url: String,
    events: Vec<String>,
}

/// Spawn a fire-and-forget task delivering `payload` to every enabled
/// integration for `env_id` that subscribes to the event. Never blocks the
/// request that triggered it.
pub fn fire_integrations(state: AppState, env_id: String, payload: Value) {
    tokio::spawn(async move {
        deliver_to_env(&state, &env_id, payload).await;
    });
}

async fn deliver_to_env(state: &AppState, env_id: &str, payload: Value) {
    let event = payload
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let rows = match sqlx::query(
        "SELECT i.id::text, i.kind, i.webhook_url, i.events, e.name AS env_name \
         FROM integrations i \
         JOIN environments e ON e.id = i.environment_id \
         WHERE i.environment_id = $1::uuid AND i.enabled = true",
    )
    .bind(env_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "Failed to query integrations for delivery");
            return;
        }
    };

    // Every row carries the same environment name (single-env query).
    let Some(first_row) = rows.first() else {
        return;
    };
    let env_name: String = first_row.get("env_name");

    let integrations: Vec<IntegrationRow> = rows
        .iter()
        .map(|r| IntegrationRow {
            id: r.get("id"),
            kind: r.get("kind"),
            webhook_url: r.get("webhook_url"),
            events: r.get("events"),
        })
        .filter(|i| wants_event(&i.events, &event))
        .collect();

    for integration in integrations {
        let Some(message) = format_for(&integration.kind, &payload, &env_name) else {
            warn!(kind = %integration.kind, "Unknown integration kind — skipping delivery");
            continue;
        };

        let body = match serde_json::to_vec(&message) {
            Ok(b) => b,
            Err(e) => {
                error!(error = %e, "Failed to serialize integration message");
                continue;
            }
        };

        deliver_with_retry(
            state,
            &integration.id,
            &integration.webhook_url,
            &body,
            &event,
            &message,
        )
        .await;
    }
}

async fn deliver_with_retry(
    state: &AppState,
    integration_id: &str,
    url: &str,
    body: &[u8],
    event: &str,
    message: &Value,
) {
    let backoff_secs = [1u64, 5, 15];

    for (attempt, &wait) in backoff_secs.iter().enumerate() {
        let req = state
            .webhook_client
            .post(url)
            .header("Content-Type", "application/json")
            .header("User-Agent", "Checkgate-Integrations/1.0")
            .body(body.to_vec());

        match req.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16() as i32;
                let body_text = resp.text().await.unwrap_or_default();
                let ok = (200..300).contains(&status);

                log_delivery(
                    &state.db,
                    integration_id,
                    event,
                    message,
                    Some(status),
                    Some(&body_text),
                    None,
                )
                .await;

                if ok {
                    info!(integration_id, status, "Integration message delivered");
                } else {
                    warn!(integration_id, status, "Integration returned non-2xx");
                }
                return;
            }
            Err(e) => {
                let is_last = attempt == backoff_secs.len().saturating_sub(1);
                if is_last {
                    let err_str = e.to_string();
                    warn!(integration_id, error = %e, "Integration delivery failed after all retries");
                    log_delivery(
                        &state.db,
                        integration_id,
                        event,
                        message,
                        None,
                        None,
                        Some(&err_str),
                    )
                    .await;
                } else {
                    warn!(integration_id, attempt, error = %e, "Integration delivery failed — retrying");
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn log_delivery(
    db: &PgPool,
    integration_id: &str,
    event: &str,
    payload: &Value,
    status_code: Option<i32>,
    response_body: Option<&str>,
    error: Option<&str>,
) {
    if let Err(e) = sqlx::query(
        "INSERT INTO integration_deliveries (integration_id, event, payload, status_code, response_body, error) \
         VALUES ($1::uuid, $2, $3, $4, $5, $6)",
    )
    .bind(integration_id)
    .bind(event)
    .bind(payload)
    .bind(status_code)
    .bind(response_body)
    .bind(error)
    .execute(db)
    .await
    {
        error!(error = %e, "Failed to log integration delivery");
    }

    // Keep the table bounded — retain the most recent 200 per integration.
    if let Err(e) = sqlx::query(
        "DELETE FROM integration_deliveries \
         WHERE integration_id = $1::uuid \
           AND id NOT IN ( \
               SELECT id FROM integration_deliveries \
               WHERE integration_id = $1::uuid \
               ORDER BY delivered_at DESC \
               LIMIT 200 \
           )",
    )
    .bind(integration_id)
    .execute(db)
    .await
    {
        warn!(error = %e, "Failed to prune old integration deliveries");
    }
}

/// Payload for change-request lifecycle events. Mirrors the shape of
/// [`crate::api::webhooks::flag_event_payload`] so both event families render
/// through the same formatter.
pub fn change_request_payload(
    event: &str,
    env_id: &str,
    flag_key: &str,
    change_request_id: i64,
    actor_email: Option<&str>,
    reason: Option<&str>,
) -> Value {
    json!({
        "event": event,
        "environment_id": env_id,
        "flag_key": flag_key,
        "change_request_id": change_request_id,
        "actor_email": actor_email,
        "metadata": { "reason": reason },
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn flag_payload(event: &str) -> Value {
        json!({
            "event": event,
            "environment_id": "env-1",
            "flag_key": "dark_mode",
            "flag": { "is_enabled": true, "rollout_percentage": 25 },
            "actor_email": "dev@example.com",
        })
    }

    #[test]
    fn empty_subscription_means_all_events() {
        assert!(wants_event(&[], "flag.created"));
        assert!(wants_event(&[], "change_request.opened"));
    }

    #[test]
    fn subscription_filters_to_listed_events() {
        let subscribed = vec!["flag.deleted".to_string()];
        assert!(wants_event(&subscribed, "flag.deleted"));
        assert!(!wants_event(&subscribed, "flag.created"));
    }

    #[test]
    fn slack_message_has_fallback_text_and_blocks() {
        let msg = format_slack(&flag_payload("flag.created"), "Production");

        // The fallback line is what shows in a push notification, so it has to
        // carry the essentials on its own.
        let text = msg["text"].as_str().unwrap();
        assert!(text.contains("Flag created"));
        assert!(text.contains("dark_mode"));
        assert!(text.contains("Production"));

        let blocks = msg["attachments"][0]["blocks"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "header");
        assert_eq!(blocks[0]["text"]["text"], "Flag created");
    }

    #[test]
    fn slack_section_stays_within_block_kit_field_limit() {
        let msg = format_slack(&flag_payload("flag.updated"), "Production");
        let fields = msg["attachments"][0]["blocks"][1]["fields"]
            .as_array()
            .unwrap();
        assert!(
            fields.len() <= 10,
            "Block Kit rejects a section with more than 10 fields, got {}",
            fields.len()
        );
    }

    #[test]
    fn slack_renders_flag_state_and_rollout() {
        let msg = format_slack(&flag_payload("flag.updated"), "Staging");
        let rendered = msg.to_string();
        assert!(rendered.contains("Enabled"));
        assert!(rendered.contains("25%"));
        assert!(rendered.contains("dev@example.com"));
    }

    #[test]
    fn slack_shows_disabled_state() {
        let mut payload = flag_payload("flag.updated");
        payload["flag"]["is_enabled"] = json!(false);
        let msg = format_slack(&payload, "Staging");
        assert!(msg.to_string().contains("Disabled"));
    }

    #[test]
    fn teams_message_uses_messagecard_schema() {
        let msg = format_teams(&flag_payload("flag.created"), "Production");

        assert_eq!(msg["@type"], "MessageCard");
        assert_eq!(msg["@context"], "https://schema.org/extensions");
        // themeColor must be a bare hex string — Teams rejects a leading '#'.
        let theme = msg["themeColor"].as_str().unwrap();
        assert!(!theme.starts_with('#'), "got {theme}");

        let facts = msg["sections"][0]["facts"].as_array().unwrap();
        assert!(
            facts
                .iter()
                .any(|f| f["name"] == "Flag" && f["value"] == "dark_mode")
        );
        assert!(
            facts
                .iter()
                .any(|f| f["name"] == "Environment" && f["value"] == "Production")
        );
    }

    #[test]
    fn slack_theme_color_is_prefixed_but_teams_is_not() {
        // Slack wants '#RRGGBB' in an attachment colour; Teams wants 'RRGGBB'.
        // Getting these backwards silently renders wrong, so pin both.
        let slack = format_slack(&flag_payload("flag.deleted"), "Prod");
        let teams = format_teams(&flag_payload("flag.deleted"), "Prod");
        assert!(
            slack["attachments"][0]["color"]
                .as_str()
                .unwrap()
                .starts_with('#')
        );
        assert!(!teams["themeColor"].as_str().unwrap().starts_with('#'));
    }

    #[test]
    fn deleted_and_rejected_events_are_red() {
        assert_eq!(event_color("flag.deleted"), "D64545");
        assert_eq!(event_color("change_request.rejected"), "D64545");
        assert_eq!(event_color("change_request.approved"), "3DA35D");
    }

    #[test]
    fn change_request_payload_renders_without_a_flag_body() {
        // Change-request events carry no `flag` object — the formatter must not
        // assume one is present.
        let payload = change_request_payload(
            "change_request.opened",
            "env-1",
            "checkout_v2",
            42,
            Some("dev@example.com"),
            None,
        );

        let slack = format_slack(&payload, "Production");
        assert!(
            slack["text"]
                .as_str()
                .unwrap()
                .contains("Change request opened")
        );
        assert!(slack.to_string().contains("checkout_v2"));

        let teams = format_teams(&payload, "Production");
        assert_eq!(teams["@type"], "MessageCard");
        assert!(teams.to_string().contains("checkout_v2"));
    }

    #[test]
    fn rejection_reason_is_surfaced() {
        let payload = change_request_payload(
            "change_request.rejected",
            "env-1",
            "checkout_v2",
            42,
            Some("lead@example.com"),
            Some("Needs a rollout plan"),
        );
        assert!(
            format_slack(&payload, "Prod")
                .to_string()
                .contains("Needs a rollout plan")
        );
        assert!(
            format_teams(&payload, "Prod")
                .to_string()
                .contains("Needs a rollout plan")
        );
    }

    #[test]
    fn empty_reason_is_omitted_rather_than_shown_blank() {
        let payload = change_request_payload(
            "change_request.rejected",
            "env-1",
            "checkout_v2",
            42,
            Some("lead@example.com"),
            Some(""),
        );
        let fields = detail_fields(&payload, "Prod");
        assert!(!fields.iter().any(|(k, _)| k == "Reason"));
    }

    #[test]
    fn unknown_provider_formats_to_none() {
        assert!(format_for("irc", &flag_payload("flag.created"), "Prod").is_none());
        assert!(format_for("slack", &flag_payload("flag.created"), "Prod").is_some());
        assert!(format_for("teams", &flag_payload("flag.created"), "Prod").is_some());
    }

    #[test]
    fn missing_optional_fields_do_not_panic() {
        // A minimal payload — no flag body, no actor, no metadata.
        let bare = json!({ "event": "flag.updated", "flag_key": "x" });
        let slack = format_slack(&bare, "Prod");
        let teams = format_teams(&bare, "Prod");
        assert!(slack["text"].as_str().unwrap().contains("Flag updated"));
        assert_eq!(teams["@type"], "MessageCard");
    }

    #[test]
    fn supported_events_all_have_titles() {
        // Guards against adding an event constant without a display title,
        // which would leak the raw dotted name into a chat message.
        for event in SUPPORTED_EVENTS {
            assert_ne!(
                event_title(event),
                *event,
                "{event} has no human-readable title"
            );
        }
    }
}
