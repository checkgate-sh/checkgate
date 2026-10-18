-- "One default environment" predates projects: the index was global, so only
-- one environment across all projects could ever be the default. Scope it to
-- the project instead.
DROP INDEX IF EXISTS environments_single_default;

CREATE UNIQUE INDEX IF NOT EXISTS environments_single_default_per_project
    ON environments (project_id)
    WHERE is_default = true;
