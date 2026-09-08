-- Persist the polling cadence observed by one tune as a typed JSON snapshot. Nullable for
-- runs created before this migration and for attempts that fail before polling begins.
ALTER TABLE tune_runs
    ADD COLUMN timing_metrics_json TEXT
        CHECK (timing_metrics_json IS NULL OR json_valid(timing_metrics_json));
