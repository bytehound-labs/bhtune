-- Initial schema for bhtune's persistence layer. Single, plain, open SQLite
-- database (see AGENTS.md's "Plain, open SQLite" design decision) -- no
-- encryption, no per-row access control, inspectable with any SQLite
-- browser or the `sqlite3` CLI.
--
-- Every table needed through the `history` phase (Phase 10 in the plan) is
-- created here, in one migration, deliberately: nothing has shipped yet, so
-- there is no meaningful "migration history" to preserve, and squashing
-- everything the roadmap already knows it needs into the initial schema
-- avoids a string of `ALTER TABLE`s later for tables whose shape is already
-- decided.
--
-- Conventions used throughout:
--   * Timestamps are TEXT (RFC 3339 / ISO 8601, UTC) -- how sqlx's `chrono`
--     feature encodes `DateTime<Utc>` for SQLite. They are always supplied
--     explicitly by the Rust layer (see `bhtune_db::pool`); `bhtune-core`
--     never reads the clock (AGENTS.md), so the orchestration layer that
--     does is also what stamps these.
--   * Enum-shaped columns store the same lowercase snake_case strings
--     `bhtune-core`'s `serde` (`rename_all = "snake_case"`) impls already
--     produce (see `bhtune_db::convert`), and are constrained with `CHECK`
--     so an invalid value can never be written by anything, not just the
--     Rust layer.
--   * Nested/optional domain data that has no meaningful SQL-level
--     filtering requirement (`LoopTags`, whose fields are individually
--     tag-or-fixed-value and template-conditional) is stored as validated
--     JSON in a single column, reusing the type's existing `serde` impl as
--     the single source of truth rather than a second, hand-maintained
--     flattening that could drift from it. Flat, stable, filterable data
--     (`LoopConfig`, initial readings) gets real columns instead.

PRAGMA foreign_keys = ON;

-- DCS/PLC vendor templates: how a control system expresses PID parameters
-- and its OPC tag-suffix naming convention. Mirrors
-- `bhtune_core::template::DcsTemplate` field-for-field.
--
-- `is_builtin = 1` rows are the four shipped presets (Yokogawa CentumVP,
-- Honeywell Experion, Schneider Modicon, Allen-Bradley PlantPAx), kept in
-- sync by `db-seed-templates` on every startup so template fixes ship via
-- app updates; `is_builtin = 0` rows are freely user-created/edited/deleted
-- and must never be touched by that sync.
CREATE TABLE dcs_templates (
    id                              INTEGER PRIMARY KEY AUTOINCREMENT,
    name                             TEXT NOT NULL UNIQUE,
    is_builtin                       INTEGER NOT NULL DEFAULT 0 CHECK (is_builtin IN (0, 1)),

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

    created_at                       TEXT NOT NULL,
    updated_at                       TEXT NOT NULL
);

-- A saved, named loop: a reusable tag mapping plus default MRFT test
-- parameters, so the CLI/GUI can re-run e.g. "Unit1.LIC101" without
-- re-entering tags each time. This is a persistence-layer DTO, not a 1:1
-- mirror of any single `bhtune-core` type -- `tags_json` is a serialized
-- `bhtune_core::tags::LoopTags` (see the file-level conventions note above),
-- while the MRFT parameter columns flatten
-- `bhtune_core::loop_config::LoopConfig` field-for-field so they stay
-- directly filterable/sortable (e.g. "loops configured for Level process
-- type") the way a JSON blob wouldn't be.
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
CREATE INDEX idx_loops_dcs_template ON loops(dcs_template_id);

-- One MRFT (or future Step Test) execution against a loop. Snapshots the
-- configuration and initial readings actually used as flattened, real
-- columns -- not a foreign key to `loops`' current config -- so editing a
-- loop's defaults later can never rewrite the historical record of what a
-- past run actually did, and so `history-query-api` can filter/sort on
-- these columns without JSON extraction.
--
-- The initial-readings columns (`pv_ini` through `controller_direction`)
-- are nullable: a row is written the moment a run is attempted, before
-- those values are read from the backend, so a failure during that very
-- read (the legacy app's `ReadInitialOPCvalues`/`InvalidCastException`
-- failure mode) still leaves an auditable "attempted at <time>, failed:
-- <reason>" record instead of silently vanishing.
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
    backend                  TEXT NOT NULL CHECK (backend IN ('opcda', 'simulator', 'replay')),

    started_at               TEXT NOT NULL,
    completed_at             TEXT,
    outcome                  TEXT NOT NULL DEFAULT 'running' CHECK (outcome IN ('running', 'completed', 'failed', 'aborted')),
    failure_reason           TEXT,

    -- Snapshot of `bhtune_core::loop_config::LoopConfig`. Always known
    -- before a run starts (user/schedule input, not backend-read), so
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

    created_at                TEXT NOT NULL
);
CREATE INDEX idx_tune_runs_loop_started ON tune_runs(loop_id, started_at);
CREATE INDEX idx_tune_runs_started_at ON tune_runs(started_at);
CREATE INDEX idx_tune_runs_outcome ON tune_runs(outcome);
CREATE INDEX idx_tune_runs_process_controller ON tune_runs(process_type, controller_type);
CREATE INDEX idx_tune_runs_template_name ON tune_runs(template_name);

-- Per-tick engine state during a run -- mirrors
-- `bhtune_core::mrft::MrftState` plus the `Tick` that produced it. This is
-- what the history explorer's trend chart reads (`history-query-api`,
-- `history-explorer-ui`). `ON DELETE CASCADE` from `tune_runs` so
-- `history-retention` deletes are a single-index cascading operation, not a
-- separate orphan-cleanup pass.
CREATE TABLE tune_samples (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id                  INTEGER NOT NULL REFERENCES tune_runs(id) ON DELETE CASCADE,
    tick                    INTEGER NOT NULL,
    time                    TEXT NOT NULL,
    pv                      REAL NOT NULL,
    hysteresis              REAL NOT NULL,
    mv_value_current        REAL NOT NULL,
    mv_sign_next_step       INTEGER NOT NULL,
    counter_all_switches    INTEGER NOT NULL,
    cycles_completed        INTEGER NOT NULL,
    cycles_remaining        INTEGER NOT NULL,
    UNIQUE (run_id, tick)
);
CREATE INDEX idx_tune_samples_run_tick ON tune_samples(run_id, tick);

-- Calculated PID results for one response level of one run -- mirrors
-- `bhtune_core::tuning_math::{TuningResult, PidParameters}`. A successfully
-- completed run always yields exactly the 3 rows in
-- `bhtune_core::constants::ResponseLevel::ALL` (Aggressive/Moderate/
-- Sluggish), written once at completion. This is what was *calculated* --
-- see `tune_writes` for what was actually *written* to the DCS, which may
-- be none, one, or more than one of these.
CREATE TABLE tune_results (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id             INTEGER NOT NULL REFERENCES tune_runs(id) ON DELETE CASCADE,
    response_level     TEXT NOT NULL CHECK (response_level IN ('aggressive', 'moderate', 'sluggish')),

    kp                  REAL NOT NULL,
    ti_minutes          REAL NOT NULL,
    td_minutes          REAL NOT NULL,

    -- `PidParameters`, in the run's DCS template's own representation.
    proportional        REAL NOT NULL,
    integral             REAL NOT NULL,
    derivative           REAL NOT NULL,

    UNIQUE (run_id, response_level)
);
CREATE INDEX idx_tune_results_run ON tune_results(run_id);

-- Audit trail of PID constants actually written back to the DCS --
-- distinct from `tune_results` (what was merely *calculated*). A row here
-- means a human (or `--write-pid` in CLI automation) actually pushed
-- values to the live controller; a run can have zero, one, or more of
-- these. This is the "who changed this loop and when" record the legacy
-- CSV logs never captured -- see `history-writeback-audit` and AGENTS.md's
-- "plain, open SQLite" philosophy: nothing here is hidden or encrypted.
CREATE TABLE tune_writes (
    id                       INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id                   INTEGER NOT NULL REFERENCES tune_runs(id) ON DELETE CASCADE,
    response_level           TEXT NOT NULL CHECK (response_level IN ('aggressive', 'moderate', 'sluggish')),
    written_at               TEXT NOT NULL,

    proportional_written     REAL NOT NULL,
    integral_written         REAL NOT NULL,
    derivative_written       REAL NOT NULL,

    -- Read back immediately after writing, to confirm the DCS actually
    -- accepted the value rather than silently clamping/rejecting it. NULL
    -- when `success = 0` and the write never got far enough to read back.
    proportional_readback    REAL,
    integral_readback        REAL,
    derivative_readback      REAL,

    success                   INTEGER NOT NULL CHECK (success IN (0, 1)),
    error_message             TEXT
);
CREATE INDEX idx_tune_writes_run ON tune_writes(run_id);

-- App-wide key/value settings -- e.g. the `history-retention` policy --
-- shared between the CLI and GUI without a dedicated table per setting.
-- `value` is a JSON-encoded scalar or object, kept as TEXT so adding a new
-- setting is never a schema migration.
CREATE TABLE settings (
    key          TEXT PRIMARY KEY,
    value        TEXT NOT NULL CHECK (json_valid(value)),
    updated_at   TEXT NOT NULL
);
