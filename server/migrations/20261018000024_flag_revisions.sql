-- Every flag write gets a revision from one global sequence. Change events
-- carry it so SDKs can drop an update that arrives after a newer one: audit
-- logging and segment expansion run after commit, so two concurrent writes to
-- the same flag can publish in the opposite order to the one they committed in.
CREATE SEQUENCE IF NOT EXISTS flag_revision_seq;

ALTER TABLE flags
    ADD COLUMN IF NOT EXISTS revision BIGINT NOT NULL DEFAULT nextval('flag_revision_seq');

-- A BEFORE UPDATE row trigger fires after the row lock is taken, so writes to
-- one flag receive increasing revisions in the order they commit.
CREATE OR REPLACE FUNCTION flags_bump_revision() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    NEW.revision := nextval('flag_revision_seq');
    RETURN NEW;
END $$;

DROP TRIGGER IF EXISTS flags_bump_revision ON flags;
CREATE TRIGGER flags_bump_revision
    BEFORE UPDATE ON flags
    FOR EACH ROW EXECUTE FUNCTION flags_bump_revision();
