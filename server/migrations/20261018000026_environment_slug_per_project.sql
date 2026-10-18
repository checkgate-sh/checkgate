-- Environment slugs were unique across all projects (the constraint predates
-- projects), so a second project could not have its own "production". Slugs
-- only need to be unique within a project.
ALTER TABLE environments DROP CONSTRAINT IF EXISTS environments_slug_key;

CREATE UNIQUE INDEX IF NOT EXISTS environments_project_slug
    ON environments (project_id, slug);
