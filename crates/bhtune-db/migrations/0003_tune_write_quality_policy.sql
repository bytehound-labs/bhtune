-- Snapshot the per-operation quality policy on every PID write audit row. The
-- policy is supplied by the caller for each operation; it is not read from the
-- parent run because a later write or revert may intentionally use a different
-- policy. The global default is true, so the migration default must also be
-- true for legacy inserts that omit the new column.
ALTER TABLE tune_writes
    ADD COLUMN allow_uncertain_quality INTEGER NOT NULL DEFAULT 1
        CHECK (allow_uncertain_quality IN (0, 1));

-- Existing rows have no per-operation value. Preserve a historically disabled
-- parent policy; rows from older runs otherwise use the new global default.
UPDATE tune_writes
SET allow_uncertain_quality = 0
WHERE run_id IN (
    SELECT id
    FROM tune_runs
    WHERE allow_uncertain_quality = 0
);
