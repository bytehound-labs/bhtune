-- Indexes added after the initial schema for the history explorer's common child-row
-- queries. This migration is intentionally small so the compatibility test can represent
-- a real pre-0002 database and prove that its data survives a forward upgrade.
CREATE INDEX IF NOT EXISTS idx_tune_samples_run_time
    ON tune_samples (run_id, time);

CREATE INDEX IF NOT EXISTS idx_tune_writes_run_written
    ON tune_writes (run_id, written_at);
