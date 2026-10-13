ALTER TABLE tune_runs ADD COLUMN recovery_state TEXT
    CHECK (recovery_state IS NULL OR recovery_state IN (
        'eligible', 'running', 'confirmed', 'incomplete', 'not_recoverable'
    ));
ALTER TABLE tune_runs ADD COLUMN recovery_evidence_json TEXT
    CHECK (recovery_evidence_json IS NULL OR json_valid(recovery_evidence_json));

CREATE TABLE live_operation_owners (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id              INTEGER REFERENCES tune_runs(id) ON DELETE CASCADE,
    operation_kind      TEXT NOT NULL CHECK (operation_kind IN (
        'tune', 'pid_write', 'pid_revert', 'recovery', 'opc_write'
    )),
    database_key        TEXT NOT NULL,
    resource_key        TEXT NOT NULL,
    owner_pid           INTEGER NOT NULL CHECK (owner_pid >= 0),
    acquired_at         TEXT NOT NULL,
    heartbeat_at        TEXT NOT NULL,
    released_at         TEXT,
    state               TEXT NOT NULL CHECK (state IN (
        'active', 'released', 'orphaned', 'recovered'
    )),
    restore_intent_json TEXT
        CHECK (restore_intent_json IS NULL OR json_valid(restore_intent_json)),
    orphan_eligible     INTEGER NOT NULL DEFAULT 0 CHECK (orphan_eligible IN (0, 1)),
    orphan_evidence_json TEXT
        CHECK (orphan_evidence_json IS NULL OR json_valid(orphan_evidence_json)),
    CHECK (
        (state = 'active' AND released_at IS NULL)
        OR (state <> 'active' AND released_at IS NOT NULL)
    ),
    CHECK (state = 'orphaned' OR orphan_eligible = 0)
);
CREATE INDEX idx_live_operation_owners_run
    ON live_operation_owners(run_id, acquired_at, id);
CREATE INDEX idx_live_operation_owners_heartbeat
    ON live_operation_owners(state, heartbeat_at);

CREATE TABLE live_operation_claims (
    claim_key   TEXT PRIMARY KEY,
    owner_id    INTEGER NOT NULL UNIQUE
        REFERENCES live_operation_owners(id) ON DELETE CASCADE,
    claimed_at  TEXT NOT NULL
);

CREATE TABLE live_mutation_steps (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    owner_id        INTEGER NOT NULL REFERENCES live_operation_owners(id) ON DELETE CASCADE,
    run_id          INTEGER REFERENCES tune_runs(id) ON DELETE CASCADE,
    step            TEXT NOT NULL CHECK (length(step) > 0),
    target_json     TEXT NOT NULL CHECK (json_valid(target_json)),
    previous_json   TEXT CHECK (previous_json IS NULL OR json_valid(previous_json)),
    status          TEXT NOT NULL CHECK (status IN (
        'intent', 'confirmed', 'failed', 'not_needed'
    )),
    started_at      TEXT NOT NULL,
    completed_at    TEXT,
    readback_json   TEXT CHECK (readback_json IS NULL OR json_valid(readback_json)),
    detail          TEXT,
    CHECK (
        (status = 'intent' AND completed_at IS NULL)
        OR (status <> 'intent' AND completed_at IS NOT NULL)
    )
);
CREATE INDEX idx_live_mutation_steps_owner
    ON live_mutation_steps(owner_id, started_at, id);
CREATE INDEX idx_live_mutation_steps_run
    ON live_mutation_steps(run_id, owner_id, step);

CREATE TABLE tune_recovery_attempts (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id              INTEGER NOT NULL REFERENCES tune_runs(id) ON DELETE CASCADE,
    source_owner_id     INTEGER NOT NULL REFERENCES live_operation_owners(id),
    recovery_owner_id   INTEGER NOT NULL UNIQUE REFERENCES live_operation_owners(id),
    export_path         TEXT NOT NULL,
    started_at          TEXT NOT NULL,
    completed_at        TEXT,
    status              TEXT NOT NULL CHECK (status IN (
        'running', 'confirmed', 'incomplete', 'failed'
    )),
    detail              TEXT,
    evidence_json       TEXT NOT NULL CHECK (json_valid(evidence_json)),
    CHECK (
        (status = 'running' AND completed_at IS NULL)
        OR (status <> 'running' AND completed_at IS NOT NULL)
    )
);
CREATE INDEX idx_tune_recovery_attempts_run
    ON tune_recovery_attempts(run_id, started_at, id);
