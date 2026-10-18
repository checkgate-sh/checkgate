-- The last revision of each deleted flag, so the SSE bootstrap can tell SDKs
-- about recent deletions. Without it, a client that bootstraps after a delete
-- has no revision for that key, and a delayed change event published before
-- the delete would bring the flag back.
--
-- No foreign key: rows are written while an environment's flags are being
-- cascade-deleted, and stale rows are pruned by age instead.
CREATE TABLE IF NOT EXISTS flag_tombstones (
    environment_id UUID        NOT NULL,
    key            TEXT        NOT NULL,
    revision       BIGINT      NOT NULL,
    deleted_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (environment_id, key)
);

CREATE INDEX IF NOT EXISTS flag_tombstones_deleted_at ON flag_tombstones (deleted_at);

-- Change events are delayed by seconds at most (post-commit audit logging and
-- segment expansion), so a day of history is ample.
CREATE OR REPLACE FUNCTION flags_record_tombstone() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO flag_tombstones (environment_id, key, revision)
    VALUES (OLD.environment_id, OLD.key, OLD.revision)
    ON CONFLICT (environment_id, key) DO UPDATE
        SET revision = GREATEST(flag_tombstones.revision, EXCLUDED.revision),
            deleted_at = NOW();
    DELETE FROM flag_tombstones WHERE deleted_at < NOW() - INTERVAL '1 day';
    RETURN OLD;
END $$;

DROP TRIGGER IF EXISTS flags_record_tombstone ON flags;
CREATE TRIGGER flags_record_tombstone
    AFTER DELETE ON flags
    FOR EACH ROW EXECUTE FUNCTION flags_record_tombstone();
