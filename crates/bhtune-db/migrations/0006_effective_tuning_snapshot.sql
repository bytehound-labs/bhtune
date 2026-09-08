-- Persist the concrete timing policy used by a tune after configuration defaults have
-- been resolved. Nullable for runs created before this migration and for callers that
-- have not yet recorded the effective values.
ALTER TABLE tune_runs
    ADD COLUMN effective_tuning_json TEXT
        CHECK (effective_tuning_json IS NULL OR json_valid(effective_tuning_json));
