-- Complete pre-v0.1 schema baseline. Existing pre-squash migration histories are
-- rejected by the database connection layer; they are never rewritten automatically.

CREATE TABLE dcs_templates (
    id                              INTEGER PRIMARY KEY AUTOINCREMENT,
    name                             TEXT NOT NULL UNIQUE,
    origin                           TEXT NOT NULL CHECK (origin IN ('builtin', 'catalog', 'user')),

    revert_mode                      INTEGER NOT NULL CHECK (revert_mode IN (0, 1)),
    proportional_type                TEXT NOT NULL CHECK (proportional_type IN ('gain', 'band')),
    integral_type                    TEXT NOT NULL CHECK (integral_type IN ('reset_time', 'reset_rate', 'reset_gain')),
    integral_unit                    TEXT NOT NULL CHECK (integral_unit IN ('seconds', 'minutes')),
    derivative_type                  TEXT NOT NULL CHECK (derivative_type IN ('derivative_time', 'derivative_gain')),
    derivative_unit                  TEXT NOT NULL CHECK (derivative_unit IN ('seconds', 'minutes')),

    process_variable_suffix          TEXT NOT NULL,
    manipulated_variable_suffix      TEXT NOT NULL,
    setpoint_variable_suffix         TEXT NOT NULL,
    controller_direction_suffix      TEXT NOT NULL,
    controller_mode_suffix           TEXT NOT NULL,
    mode_attribute_suffix            TEXT NOT NULL,
    upper_pv_range_suffix            TEXT NOT NULL,
    lower_pv_range_suffix            TEXT NOT NULL,
    upper_mv_range_suffix            TEXT NOT NULL,
    lower_mv_range_suffix            TEXT NOT NULL,
    proportional_constant_suffix     TEXT NOT NULL,
    integral_constant_suffix         TEXT NOT NULL,
    derivative_constant_suffix       TEXT NOT NULL,

    mode_manual_value                TEXT NOT NULL,
    mode_auto_value                  TEXT NOT NULL,
    mode_attribute_program_value     TEXT,
    controller_action_direct_value   TEXT NOT NULL,

    -- The DCS/PLC releases this template's tag conventions are known to
    -- apply to (`bhtune_core::template::DcsTemplate::versions`, a JSON
    -- array of strings -- no independent identity or SQL-filtering need of
    -- its own, same reasoning as `loops.tags_json`), a short human
    -- description, and a documentation citation for where the mapping came
    -- from. All optional: a template is usable without any of them.
    versions_json                    TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(versions_json)),
    description                      TEXT,
    source                           TEXT,

    created_at                       TEXT NOT NULL,
    updated_at                       TEXT NOT NULL,
    pid_rounding_kind                TEXT NOT NULL DEFAULT 'significant_digits'
        CHECK (pid_rounding_kind IN ('decimal_places', 'significant_digits')),
    pid_rounding_digits              INTEGER NOT NULL DEFAULT 3
        CHECK (
        typeof(pid_rounding_digits) = 'integer'
        AND pid_rounding_digits BETWEEN 0 AND 7
        AND (pid_rounding_kind = 'decimal_places' OR pid_rounding_digits >= 1)
    )
);

CREATE TABLE demo_sessions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    token_hash    TEXT NOT NULL UNIQUE,
    created_at    TIMESTAMP NOT NULL,
    last_seen_at  TIMESTAMP NOT NULL,
    expires_at    TIMESTAMP NOT NULL,
    revoked_at    TIMESTAMP,
    CHECK (length(token_hash) = 64 AND token_hash NOT GLOB '*[^0-9a-f]*'),
    CHECK (expires_at > created_at),
    CHECK (revoked_at IS NULL OR revoked_at >= created_at)
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

CREATE TABLE live_operation_claims (
    claim_key   TEXT PRIMARY KEY,
    owner_id    INTEGER NOT NULL UNIQUE
        REFERENCES live_operation_owners(id) ON DELETE CASCADE,
    claimed_at  TEXT NOT NULL
);

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

CREATE TABLE loops (
    id                        INTEGER PRIMARY KEY AUTOINCREMENT,
    name                      TEXT NOT NULL UNIQUE,
    dcs_template_id           INTEGER NOT NULL REFERENCES dcs_templates(id) ON DELETE RESTRICT,

    -- Serialized `bhtune_core::tags::LoopTags`.
    tags_json                 TEXT NOT NULL CHECK (json_valid(tags_json)),

    -- Flattened `bhtune_core::loop_config::LoopConfig` -- last-used/default
    -- MRFT parameters, pre-filled next time this loop is selected. Each
    -- `tune_runs` row snapshots its own copy at run time (see below); this
    -- is only ever "the current default", never a historical record.
    process_type              TEXT NOT NULL CHECK (process_type IN ('flow', 'pressure_line', 'pressure_vessel', 'level', 'temperature_mixing', 'temperature_heat_exchange')),
    controller_type           TEXT NOT NULL CHECK (controller_type IN ('p', 'pi', 'pid')),
    relay_amp_percent         REAL NOT NULL,
    num_cycles_skip           INTEGER NOT NULL,
    num_cycles_count          INTEGER NOT NULL,
    noise_protection_secs     INTEGER NOT NULL,
    mrft_delay_secs           INTEGER NOT NULL,

    created_at                TEXT NOT NULL,
    updated_at                TEXT NOT NULL
);

CREATE TABLE settings (
    key          TEXT PRIMARY KEY,
    value        TEXT NOT NULL CHECK (json_valid(value)),
    updated_at   TEXT NOT NULL
);

CREATE TABLE tune_mv_actuations (
    id                       INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id                   INTEGER NOT NULL REFERENCES tune_runs(id) ON DELETE CASCADE,
    sequence                 INTEGER NOT NULL CHECK (sequence >= 0),
    kind                     TEXT NOT NULL CHECK (kind IN ('relay', 'restore')),
    commanded_at             TEXT NOT NULL,
    target_mv                REAL NOT NULL,
    previous_commanded_mv    REAL,
    tolerance                REAL NOT NULL CHECK (tolerance >= 0),
    confirmation_due_at      TEXT NOT NULL,
    last_checked_at          TEXT,
    readback_mv              REAL,
    readback_quality         TEXT CHECK (
        readback_quality IS NULL
        OR readback_quality IN ('good', 'uncertain', 'bad')
    ),
    attempt_count            INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    status                   TEXT NOT NULL DEFAULT 'pending' CHECK (
        status IN ('pending', 'confirmed', 'failed', 'unverified', 'superseded')
    ),
    detail                   TEXT,
    UNIQUE (run_id, sequence)
);

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

CREATE TABLE tune_results (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id             INTEGER NOT NULL REFERENCES tune_runs(id) ON DELETE CASCADE,
    response_level     TEXT NOT NULL CHECK (response_level IN ('aggressive', 'moderate', 'sluggish')),

    kp                 REAL,
    ti_minutes         REAL,
    td_minutes         REAL,

    -- `PidParameters`, in the run's DCS template's own representation.
    proportional        REAL,
    integral             REAL,
    derivative           REAL,

    status             TEXT NOT NULL DEFAULT 'valid',
    invalid_reason     TEXT CHECK (
        invalid_reason IS NULL OR invalid_reason IN (
            'non_finite_pv_amplitude',
            'non_positive_pv_amplitude',
            'non_finite_period',
            'non_positive_period',
            'non_finite_frequency',
            'non_positive_frequency',
            'non_finite_kp',
            'non_finite_ti_minutes',
            'non_finite_td_minutes',
            'non_finite_proportional',
            'non_finite_integral',
            'non_finite_derivative'
        )
    ),

    CHECK (
        (status = 'valid'
            AND invalid_reason IS NULL
            AND kp IS NOT NULL
            AND ti_minutes IS NOT NULL
            AND td_minutes IS NOT NULL
            AND proportional IS NOT NULL
            AND integral IS NOT NULL
            AND derivative IS NOT NULL)
        OR
        (status = 'invalid'
            AND invalid_reason IS NOT NULL
            AND kp IS NULL
            AND ti_minutes IS NULL
            AND td_minutes IS NULL
            AND proportional IS NULL
            AND integral IS NULL
            AND derivative IS NULL)
    ),
    CHECK (kp IS NULL OR kp BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38),
    CHECK (ti_minutes IS NULL OR ti_minutes BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38),
    CHECK (td_minutes IS NULL OR td_minutes BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38),
    CHECK (proportional IS NULL OR proportional BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38),
    CHECK (integral IS NULL OR integral BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38),
    CHECK (derivative IS NULL OR derivative BETWEEN -3.4028234663852886e38 AND 3.4028234663852886e38),

    UNIQUE (run_id, response_level)
);

CREATE TABLE tune_runs (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    loop_id                INTEGER REFERENCES loops(id) ON DELETE SET NULL,
    -- Denormalized snapshot: survives the loop being renamed or deleted,
    -- since a historical run should still show what it was run against.
    loop_name               TEXT NOT NULL,

    -- Snapshot of the DCS/PLC template (and the tags resolved from it) this
    -- run was actually configured against, so a historical run stays
    -- interpretable after the template catalog changes underneath it --
    -- renamed, edited, or deleted (see `template-provenance`). Deliberately
    -- not a foreign key to `dcs_templates`, unlike `loops.dcs_template_id`:
    -- a run must remain readable even once the referenced template row is
    -- gone. `template_name`/`template_origin` are flat, denormalized copies
    -- of fields already inside `template_snapshot_json`, present purely so
    -- they're filterable/indexable without `json_extract`; the JSON blobs
    -- are what's actually deserialized back into `DcsTemplate`/`LoopTags`
    -- for display or reproduction.
    template_name           TEXT NOT NULL,
    template_origin         TEXT NOT NULL CHECK (template_origin IN ('builtin', 'catalog', 'user')),
    template_snapshot_json  TEXT NOT NULL CHECK (json_valid(template_snapshot_json)),
    -- Serialized `bhtune_core::tags::LoopTags` this run actually resolved
    -- and used -- same shape as `loops.tags_json`, but a point-in-time
    -- snapshot rather than a live "current" mapping.
    tags_json               TEXT NOT NULL CHECK (json_valid(tags_json)),

    test_type               TEXT NOT NULL DEFAULT 'mrft' CHECK (test_type IN ('mrft')),
    driver                  TEXT NOT NULL CHECK (driver IN ('opcda', 'simulator', 'replay')),
    -- The OPC DA server ProgID and opcda-bridge gateway host this run actually used --
    -- `NULL`/`NULL` for a non-opcda run, which has neither. Flat and filterable rather than
    -- folded into `request_json` below, because `bhtune history revert` must know exactly
    -- which plant a past run touched: re-resolving the server/host from the *current*
    -- `--server`/`--bridge-host`/config at revert time (as it did before this column
    -- existed) can silently revert a loop against a different DCS than the run actually
    -- wrote to, using tag names that may well exist on both. Set via a follow-up
    -- `record_connection` call rather than a `start()` parameter -- see that method's doc
    -- comment -- matching `allow_uncertain_quality`'s precedent below.
    opc_server               TEXT,
    bridge_host              TEXT,
    -- The complete run request exactly as submitted (CLI flags or the HTTP `POST /api/runs`
    -- body), before any config-driven defaulting -- e.g. `cycles_skip`/`cycles_count`/
    -- `noise_protection_secs` stay absent here even when `process_type` supplies a default
    -- for them. Powers `ui-prefill-last-run` (seed the New Run form from the newest run)
    -- and "duplicate this run"; the flat columns above remain the source of truth for
    -- safety-critical connection facts, never this blob. Defaults to `'{}'` so `start()`'s
    -- INSERT (which predates this column) keeps compiling and succeeding unchanged; the one
    -- production caller (`bhtune-cli`'s `prepare()`) overwrites it via `record_connection`
    -- immediately afterwards, before any driver I/O.
    request_json             TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(request_json)),
    -- Freeform operator notes attached to this run. Unlike `request_json`, this is mutable:
    -- the GUI can add, edit, or clear it while a run is active or after it finishes.
    notes                    TEXT,
    -- Historical per-run snapshot of the global TOML quality policy. The pre-Config schema
    -- default remains false so rows inserted by older binaries retain their original meaning;
    -- new production runs record the live global default explicitly.
    allow_uncertain_quality       INTEGER NOT NULL DEFAULT 0 CHECK (allow_uncertain_quality IN (0, 1)),

    started_at               TEXT NOT NULL,
    completed_at             TEXT,
    outcome                  TEXT NOT NULL DEFAULT 'running' CHECK (outcome IN ('running', 'completed', 'failed', 'aborted')),
    failure_reason           TEXT,

    -- Snapshot of `bhtune_core::loop_config::LoopConfig`. Always known
    -- before a run starts (user/schedule input, not driver-read), so
    -- these stay `NOT NULL` even though the initial-readings columns below
    -- don't.
    process_type             TEXT NOT NULL CHECK (process_type IN ('flow', 'pressure_line', 'pressure_vessel', 'level', 'temperature_mixing', 'temperature_heat_exchange')),
    controller_type          TEXT NOT NULL CHECK (controller_type IN ('p', 'pi', 'pid')),
    relay_amp_percent        REAL NOT NULL,
    num_cycles_skip          INTEGER NOT NULL,
    num_cycles_count         INTEGER NOT NULL,
    noise_protection_secs    INTEGER NOT NULL,
    mrft_delay_secs          INTEGER NOT NULL,

    -- Snapshot of `bhtune_core::mrft::InitialReadings` plus the PV range
    -- and resolved direction `core-tuning-math` needs alongside it. See
    -- the nullability note above.
    pv_ini                    REAL,
    mv_ini                    REAL,
    mv_range_low              REAL,
    mv_range_high             REAL,
    pv_range_high             REAL,
    pv_range_low              REAL,
    controller_direction      TEXT CHECK (controller_direction IS NULL OR controller_direction IN ('direct', 'reverse')),
    -- Raw mode/mode-attribute tag values and the captured setpoint, read
    -- (never written) before `transition_to_manual`'s first mutating write
    -- and persisted here in the same call as the columns above -- i.e.
    -- before the loop is touched at all (`safety-restore-guard`, finding 3
    -- of the live-plant safety review). This is what lets a crashed run be
    -- reconstructed and restored later via `bhtune restore-loop`, without
    -- needing to have survived to record anything else.
    mode_raw                  TEXT,
    mode_attribute_raw        TEXT,
    setpoint_ini              REAL,

    -- Outcome of the best-effort restore attempted after this run ended
    -- (`safety-restore-guard`). NULL means no restore was ever attempted --
    -- either the run never mutated the loop in the first place, or it's
    -- still genuinely in progress; `restore_detail` is only ever set
    -- alongside `incomplete`, naming what a second Ctrl+C,
    -- `[tuning].restore_timeout_secs`, or an individual failed restore step
    -- prevented from being confirmed.
    restore_status             TEXT CHECK (restore_status IS NULL OR restore_status IN ('confirmed', 'incomplete')),
    restore_detail              TEXT,

    created_at                TEXT NOT NULL,
    timing_metrics_json      TEXT
        CHECK (timing_metrics_json IS NULL OR json_valid(timing_metrics_json)),
    effective_tuning_json    TEXT
        CHECK (effective_tuning_json IS NULL OR json_valid(effective_tuning_json)),
    demo_session_id          INTEGER
        REFERENCES demo_sessions(id) ON DELETE CASCADE,
    gateway_compatibility_json TEXT
        CHECK (
            gateway_compatibility_json IS NULL
            OR json_valid(gateway_compatibility_json)
        ),
    recovery_state           TEXT
        CHECK (recovery_state IS NULL OR recovery_state IN (
            'eligible', 'running', 'confirmed', 'incomplete', 'not_recoverable'
        )),
    recovery_evidence_json   TEXT
        CHECK (recovery_evidence_json IS NULL OR json_valid(recovery_evidence_json))
);

CREATE TABLE tune_samples (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id                  INTEGER NOT NULL REFERENCES tune_runs(id) ON DELETE CASCADE,
    tick                    INTEGER NOT NULL,
    time                    TEXT NOT NULL,
    pv                      REAL NOT NULL,
    -- Driver-reported quality of this tick's `pv` reading (finding 5 of the
    -- live-plant safety review). A non-`Good` sample (unless the global
    -- `allow_uncertain_quality` policy permits this `uncertain` reading) aborts the
    -- run before this row is even the last one written -- see
    -- `run_polling_loop` in bhtune-cli -- so most rows here are `good`, and
    -- a non-`good` row is always the final sample of an aborted run.
    pv_quality              TEXT NOT NULL CHECK (pv_quality IN ('good', 'uncertain', 'bad')),
    hysteresis              REAL NOT NULL,
    mv_value_current        REAL NOT NULL,
    mv_sign_next_step       INTEGER NOT NULL,
    counter_all_switches    INTEGER NOT NULL,
    cycles_completed        INTEGER NOT NULL,
    cycles_remaining        INTEGER NOT NULL,
    UNIQUE (run_id, tick)
);

CREATE TABLE tune_writes (
    id                       INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id                   INTEGER NOT NULL REFERENCES tune_runs(id) ON DELETE CASCADE,
    response_level           TEXT NOT NULL CHECK (response_level IN ('aggressive', 'moderate', 'sluggish')),
    written_at               TEXT NOT NULL,
    -- 'write' is a normal write-back of freshly calculated PID parameters
    -- (`maybe_write_back`); 'revert' is `bhtune history revert` writing a
    -- previous 'write' row's `*_previous` values back, undoing it. Both
    -- kinds share this table since they're the same shape (pre-read, write
    -- and verify each constant, audit the outcome) -- only which values are
    -- being written differs.
    kind                     TEXT NOT NULL CHECK (kind IN ('write', 'revert')),

    -- Read back *before* any write is attempted, so the pre-write state is
    -- always known if a rollback later turns out to be necessary. NULL only
    -- when the pre-read itself failed, in which case no write was attempted
    -- at all (`success = 0`, every `*_written`/`*_readback` column NULL too)
    -- -- see `safety-writeback-rollback`.
    proportional_previous    REAL,
    integral_previous        REAL,
    derivative_previous      REAL,

    -- NULL when the pre-read failed and no write was attempted for that
    -- constant. Populated in write order (P, then I, then D), so a partial
    -- failure leaves later constants NULL rather than 0.
    proportional_written     REAL,
    integral_written         REAL,
    derivative_written       REAL,

    -- Read back immediately after writing, to confirm the DCS actually
    -- accepted the value rather than silently clamping/rejecting it, within
    -- tolerance. NULL whenever the corresponding `*_written` column is NULL.
    proportional_readback    REAL,
    integral_readback        REAL,
    derivative_readback      REAL,

    success                   INTEGER NOT NULL CHECK (success IN (0, 1)),
    error_message             TEXT,

    -- Set only when `success = 0` and at least one constant had already been
    -- written before the failure, so a best-effort rollback to the
    -- `*_previous` values was attempted. NULL means "no rollback was
    -- applicable" -- covers both a fully successful write (nothing to roll
    -- back) and a pre-read failure (nothing was ever written). Always NULL
    -- for a `kind = 'revert'` row -- a revert does not itself chain into a
    -- further rollback. See `bhtune history revert` for reverting a
    -- *successful* write later.
    rollback_state            TEXT CHECK (rollback_state IN ('succeeded', 'failed')),
    rollback_error            TEXT,
    allow_uncertain_quality   INTEGER NOT NULL DEFAULT 1
        CHECK (allow_uncertain_quality IN (0, 1))
);

CREATE INDEX idx_demo_sessions_expires_at ON demo_sessions(expires_at);

CREATE INDEX idx_live_mutation_steps_owner
    ON live_mutation_steps(owner_id, started_at, id);

CREATE INDEX idx_live_mutation_steps_run
    ON live_mutation_steps(run_id, owner_id, step);

CREATE INDEX idx_live_operation_owners_heartbeat
    ON live_operation_owners(state, heartbeat_at);

CREATE INDEX idx_live_operation_owners_run
    ON live_operation_owners(run_id, acquired_at, id);

CREATE INDEX idx_loops_dcs_template ON loops(dcs_template_id);

CREATE INDEX idx_tune_recovery_attempts_run
    ON tune_recovery_attempts(run_id, started_at, id);

CREATE INDEX idx_tune_results_run ON tune_results(run_id);

CREATE INDEX idx_tune_runs_demo_session
    ON tune_runs(demo_session_id, started_at DESC);

CREATE INDEX idx_tune_runs_demo_session_outcome
    ON tune_runs(demo_session_id, outcome);

CREATE INDEX idx_tune_runs_loop_started ON tune_runs(loop_id, started_at);

CREATE INDEX idx_tune_runs_outcome ON tune_runs(outcome);

CREATE INDEX idx_tune_runs_process_controller ON tune_runs(process_type, controller_type);

CREATE INDEX idx_tune_runs_started_at ON tune_runs(started_at);

CREATE INDEX idx_tune_runs_template_name ON tune_runs(template_name);

CREATE INDEX idx_tune_samples_run_tick ON tune_samples(run_id, tick);

CREATE INDEX idx_tune_samples_run_time
    ON tune_samples (run_id, time);

CREATE INDEX idx_tune_writes_run ON tune_writes(run_id);

CREATE INDEX idx_tune_writes_run_written
    ON tune_writes (run_id, written_at);

CREATE TRIGGER tune_runs_demo_global_limit_insert
BEFORE INSERT ON tune_runs
WHEN NEW.demo_session_id IS NOT NULL
 AND (SELECT COUNT(*) FROM tune_runs WHERE demo_session_id IS NOT NULL) >= 5000
BEGIN
    SELECT RAISE(ABORT, 'demo tune run row limit reached');
END;

CREATE TRIGGER tune_runs_demo_session_immutable
BEFORE UPDATE OF demo_session_id ON tune_runs
WHEN NEW.demo_session_id IS NOT OLD.demo_session_id
BEGIN
    SELECT RAISE(ABORT, 'demo_session_id is immutable');
END;

CREATE TRIGGER tune_runs_demo_session_insert
BEFORE INSERT ON tune_runs
WHEN NEW.demo_session_id IS NOT NULL AND NEW.driver <> 'simulator'
BEGIN
    SELECT RAISE(ABORT, 'demo_session_id requires simulator driver');
END;

CREATE TRIGGER tune_runs_demo_session_update
BEFORE UPDATE OF driver ON tune_runs
WHEN NEW.demo_session_id IS NOT NULL AND NEW.driver <> 'simulator'
BEGIN
    SELECT RAISE(ABORT, 'demo_session_id requires simulator driver');
END;

CREATE TRIGGER tune_runs_demo_session_valid_insert
BEFORE INSERT ON tune_runs
WHEN NEW.demo_session_id IS NOT NULL AND NOT EXISTS (
    SELECT 1
    FROM demo_sessions
    WHERE id = NEW.demo_session_id
      AND revoked_at IS NULL
      AND created_at <= NEW.started_at
      AND expires_at > NEW.started_at
)
BEGIN
    SELECT RAISE(ABORT, 'demo_session_id requires an active demo session');
END;
