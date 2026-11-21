-- A third personal-access-token scope, `propose`: reads, plus proposing flag
-- changes that a different person must approve (every PATCH becomes a pending
-- change request, creates are new + disabled only, and the token may withdraw
-- its owner's own pending requests). Meant for AI agents and other
-- automation that must never apply, approve or reject a change by itself.
--
-- Additive: existing rows keep their scope and the default stays
-- `read_write`. The CHECK constraint is replaced, not dropped, so the column
-- still accepts only known scopes. A binary older than this migration treats
-- any scope other than `read_write` as read-only, so a `propose` token
-- presented to one cannot write at all (fails closed). Note that an older
-- binary refuses to boot against a database that has this migration applied
-- (the migrator reports it as missing); rolling back means restoring the
-- pre-upgrade database or deleting this row from `_sqlx_migrations`.
--
-- The original inline CHECK got PostgreSQL's default name; drop whichever
-- CHECK constraint covers `scope`, so a database whose constraint was named
-- differently does not keep rejecting `propose`.
DO $$
DECLARE c record;
BEGIN
    FOR c IN
        SELECT con.conname
        FROM pg_constraint con
        JOIN pg_attribute att
          ON att.attrelid = con.conrelid AND att.attnum = ANY (con.conkey)
        WHERE con.conrelid = 'personal_access_tokens'::regclass
          AND con.contype = 'c'
          AND att.attname = 'scope'
    LOOP
        EXECUTE format('ALTER TABLE personal_access_tokens DROP CONSTRAINT %I', c.conname);
    END LOOP;
END $$;
ALTER TABLE personal_access_tokens
    ADD CONSTRAINT personal_access_tokens_scope_check
    CHECK (scope IN ('read_only', 'read_write', 'propose'));
