-- Creating a flag in an environment that requires approval goes through a
-- change request, like every other flag change there. Before this, an editor
-- could POST an enabled flag straight into such an environment: SDKs were
-- served it at once, with no review.
--
-- `action` says what an approval does: 'update' (every row before this
-- migration, and every PATCH) merges `patch` onto the existing flag; 'create'
-- inserts the flag that `patch` holds in full (the POST body: the flag plus
-- `tags` and `owner_email`). Nothing is written to `flags` until a different
-- person approves the request, so a pending or rejected creation is never
-- served to SDKs.
--
-- Additive: existing rows become 'update', which is what they are. A binary
-- older than this migration ignores the column; it would treat a pending
-- 'create' row as a patch of a flag that does not exist (approval answers
-- 404, nothing is written), so it fails closed. The upgrade guide still says
-- to cancel pending creations before rolling back.
ALTER TABLE change_requests
    ADD COLUMN IF NOT EXISTS action TEXT NOT NULL DEFAULT 'update'
        CONSTRAINT change_requests_action_check CHECK (action IN ('update', 'create'));

-- At most one pending creation per key and environment. Automation that
-- retries a create (the Kubernetes operator requeues, Terraform re-applies)
-- gets 409 instead of piling up duplicate requests, and of two people
-- proposing the same new key at once, exactly one request is stored.
CREATE UNIQUE INDEX IF NOT EXISTS idx_change_requests_one_pending_create
    ON change_requests (environment_id, flag_key)
    WHERE status = 'pending' AND action = 'create';
