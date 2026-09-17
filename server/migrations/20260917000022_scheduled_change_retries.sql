-- Failed jobs remain pending and expose their most recent failure for inspection.
ALTER TABLE scheduled_changes
    ADD COLUMN attempts BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN last_attempt_at TIMESTAMPTZ,
    ADD COLUMN last_error TEXT;
