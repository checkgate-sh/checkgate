-- Archiving or unarchiving a flag is dashboard metadata: it publishes no
-- change event, because nothing an SDK evaluates changed. It must not take a
-- new revision either, or the committed revision would run ahead of every
-- revision ever delivered, and reconciliation (server/src/sync.rs) would read
-- that as a lost change event and resync the environment's clients.
--
-- Every other update still takes a new revision, including the no-op
-- `SET revision = revision` that re-revisions flags after a segment change.
CREATE OR REPLACE FUNCTION flags_bump_revision() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.archived_at IS DISTINCT FROM OLD.archived_at
       AND (to_jsonb(NEW) - 'archived_at' - 'revision')
           = (to_jsonb(OLD) - 'archived_at' - 'revision') THEN
        NEW.revision := OLD.revision;
        RETURN NEW;
    END IF;
    NEW.revision := nextval('flag_revision_seq');
    RETURN NEW;
END $$;
