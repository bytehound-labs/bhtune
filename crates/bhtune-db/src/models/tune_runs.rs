use bhtune_core::{
    ControllerDirection, ControllerType, DcsTemplate, LoopConfig, LoopTags, ProcessType,
};
use chrono::{DateTime, Utc};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool, sqlite::SqliteRow};

use crate::{
    convert::{enum_to_text, json_text, text_to_enum},
    error::{DbError, DbResult},
};

use super::dcs_templates::TemplateOrigin;

#[cfg(doc)]
use super::{dcs_templates::DcsTemplateRow, loops::LoopRow, tune_results::TuneResultRow};

// tune_runs {{{1

/// Which [`crate`]-agnostic I/O driver a run used. Lives in `bhtune-db` rather than
/// `bhtune-core` because it's a persistence/orchestration concept (which adapter drove this
/// run), not a domain concept the pure MRFT engine itself needs to know about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TuneDriver {
    Opcda,
    Simulator,
    Replay,
}

/// A run's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TuneOutcome {
    Running,
    Completed,
    Failed,
    Aborted,
}

/// The clock basis used for a run's persisted polling-cadence diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TimingBasis {
    /// Simulator process evolution and MRFT timestamps both advance by one exact configured
    /// poll interval per successful PV sample.
    SimulatedFixedStep,
    /// Live OPC DA timestamps are UTC projections of monotonic elapsed time, preserving real
    /// scheduling and driver delays without exposure to wall-clock adjustments.
    LiveMonotonic,
}

/// Advisory assessment of whether the recorded samples provide enough observations per
/// oscillation period for a trustworthy extrema measurement.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SamplingAdequacy {
    /// At least the documented minimum number of samples per measured oscillation period.
    Adequate,
    /// Fewer than the documented minimum number of samples per measured oscillation period.
    Marginal,
    /// No finite, positive oscillation period was available for this assessment.
    #[default]
    NotAssessed,
}

/// Summary statistics for one class of measured polling work.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct TimingSummary {
    pub count: u64,
    pub mean_ms: Option<f64>,
    pub max_ms: Option<f64>,
}

/// Operation-latency diagnostics captured while a run is polling.
///
/// Each category counts only operations that completed successfully. A zero count with
/// `None` mean/max means that the operation did not occur during the run. Categories can
/// overlap: while a relay command is pending, one batched OPC read supplies both the PV sample
/// and MV verification, so its elapsed duration may appear in both summaries and must not be
/// added twice as independent I/O time.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct PollLatencyMetrics {
    pub pv_read: TimingSummary,
    pub mv_write: TimingSummary,
    pub mv_verification: TimingSummary,
    pub sample_persist: TimingSummary,
    pub tick_work: TimingSummary,
}

/// Polling-cadence diagnostics captured over one run's successful PV samples.
///
/// The two optional gap fields are `None` when fewer than two samples were observed. The
/// measured oscillation fields are populated only for a completed MRFT run. Sampling adequacy
/// is advisory metadata and does not determine calculated-result validity or write eligibility.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct TimingMetrics {
    pub basis: TimingBasis,
    pub requested_interval_ms: u64,
    pub sample_gap_count: u64,
    pub mean_sample_gap_ms: Option<f64>,
    pub max_sample_gap_ms: Option<f64>,
    /// Number of adjacent sample gaps at least twice the requested interval. Each such gap
    /// proves that at least one complete polling opportunity was missed.
    pub missed_poll_opportunity_count: u64,
    pub measured_oscillation_period_ms: Option<f64>,
    pub approximate_samples_per_period: Option<f64>,
    /// Assessment uses the observed samples-per-period value and a six-sample advisory
    /// threshold. Old timing snapshots deserialize as `not_assessed`.
    #[serde(default)]
    pub sampling_adequacy: SamplingAdequacy,
    /// Detailed operation timings. Old timing snapshots may not contain this field.
    #[serde(default)]
    pub poll_latency: Option<PollLatencyMetrics>,
}

/// Concrete tune timing values after configuration defaults have been resolved.
///
/// Stored as one compact snapshot because these values are consumed together and have no
/// SQL-level filtering requirement. `None` on [`TuneRunRow::effective_tuning`] identifies
/// runs created before this snapshot existed or callers that have not recorded it yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct EffectiveTuning {
    pub mrft_delay_secs: u32,
    pub poll_interval_ms: u64,
    pub timeout_secs: u64,
    pub op_timeout_secs: u64,
    pub restore_timeout_secs: u64,
}

/// The outcome of a best-effort loop-restore attempt made after a run ended --
/// `safety-restore-guard` (finding 3 of the live-plant safety review). Recorded via
/// [`TuneRunRow::record_restore_status`]; `NULL` in the database (mapped to `None` on
/// [`TuneRunRow::restore_status`]) means no restore was ever attempted -- either the run
/// never mutated the loop at all, or it hasn't ended yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RestoreStatus {
    /// `restore()` ran every applicable step to completion with no failures.
    Confirmed,
    /// A second Ctrl+C arrived, `[tuning].restore_timeout_secs` elapsed, or one or more
    /// individual restore steps themselves failed, before the restore could be confirmed
    /// complete. The loop may still be at a relay-test MV/mode -- see `restore_detail` for
    /// what an operator (or `bhtune restore-loop`) needs to check by hand.
    Incomplete,
}

/// The initial-readings snapshot for a [`TuneRunRow`] — known only once the driver's initial
/// read actually succeeds (`ReadInitialOPCvalues` in the legacy app); `None` for a run that
/// failed before or during that step. Combines
/// [`bhtune_core::mrft::InitialReadings`]/[`bhtune_core::range::PvRange`] with the
/// resolved [`ControllerDirection`] `core-tuning-math` needs alongside them, as one bespoke
/// type, since gluing the two existing structs together with one extra field isn't any
/// simpler than a purpose-built one here.
#[derive(Debug, Clone, PartialEq)]
pub struct TuneRunInitialReadings {
    pub pv_ini: f32,
    pub mv_ini: f32,
    pub mv_range_low: f32,
    pub mv_range_high: f32,
    pub pv_range_high: f32,
    pub pv_range_low: f32,
    pub controller_direction: ControllerDirection,
    /// The controller mode tag's raw value at read time, before any mutation --
    /// `None` when the template/loop has no mode tag at all. Persisted (rather than kept
    /// only in-process) so a crashed run's restore intent survives the process dying
    /// outright -- `safety-restore-guard` (finding 3 of the live-plant safety review).
    pub mode_raw: Option<String>,
    /// The mode-attribute tag's raw value at read time, before any mutation -- `None` when
    /// the template/loop has no mode-attribute tag at all. See `mode_raw`.
    pub mode_attribute_raw: Option<String>,
    /// The setpoint read while the loop was still in its original mode, captured only when
    /// that mode was Auto (mirrors `SvValueIni` in the legacy app) -- `None` otherwise. Read
    /// here rather than during the mode transition itself (unlike the legacy app), since a
    /// plain read has no mutation risk and can safely happen before the loop is touched at
    /// all. See `mode_raw`.
    pub setpoint_ini: Option<f32>,
}

/// One row of `tune_runs`: a single MRFT (or future Step Test) execution against a loop.
#[derive(Debug, Clone, PartialEq)]
pub struct TuneRunRow {
    pub id: i64,
    pub loop_id: Option<i64>,
    pub demo_session_id: Option<i64>,
    pub loop_name: String,
    pub driver: TuneDriver,
    /// The OPC DA server ProgID this run actually used -- `None` for a non-opcda run, or for
    /// any run started before [`TuneRunRow::record_connection`] is called (see that method's
    /// doc comment). Flat and filterable rather than folded into `request_json`, since
    /// `bhtune history revert` must know exactly which plant a past run touched.
    pub opc_server: Option<String>,
    /// The opcda-bridge gateway host this run actually used -- `None` for a non-opcda run,
    /// or before [`TuneRunRow::record_connection`] is called. See `opc_server`.
    pub bridge_host: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub outcome: TuneOutcome,
    pub failure_reason: Option<String>,
    /// Snapshot of the `LoopConfig` this run was started with — always known up front, since
    /// it's user/schedule input rather than something read from the driver.
    pub config: LoopConfig,
    /// Where the snapshotted `template` below came from (see [`TemplateOrigin`]).
    pub template_origin: TemplateOrigin,
    /// Snapshot of the exact [`DcsTemplate`] this run was configured against, deserialized
    /// from `template_snapshot_json`. Held as the full struct rather than just its `name` --
    /// which is what makes a historical run stay interpretable once the template catalog
    /// changes underneath it (`safety-run-snapshot`). There's no separate `template_name`
    /// field here even though the table has a `template_name` column: `.name` on this field
    /// already carries that value, and the column exists purely so it's filterable/indexable
    /// without `json_extract` (see this module's own doc comment).
    pub template: DcsTemplate,
    /// Snapshot of the resolved [`LoopTags`] this run actually used, deserialized from
    /// `tags_json`.
    pub tags: LoopTags,
    /// The complete run request exactly as submitted (CLI flags or the HTTP
    /// `POST /api/runs` body), before any config-driven defaulting -- raw JSON rather than a
    /// typed struct, since its shape is owned by the `bhtune`/`bhtune-server` adapters, not
    /// `bhtune-db`. `"{}"` for any run started before
    /// [`TuneRunRow::record_connection`] is called. Powers `ui-prefill-last-run` and
    /// "duplicate this run"; never treat this as the source of truth for connection
    /// facts -- that's `opc_server`/`bridge_host` above.
    pub request_json: String,
    /// Mutable operator notes for this run. `None` means no note is recorded.
    pub notes: Option<String>,
    pub initial_readings: Option<TuneRunInitialReadings>,
    /// Whether this run permitted `Quality::Uncertain` OPC readings under the global
    /// `allow_uncertain_quality` policy (finding 5 of the live-plant safety review;
    /// `Quality::Bad` is never accepted regardless). `false` for every run started before
    /// [`TuneRunRow::record_allow_uncertain_quality`] is called -- see that method's doc
    /// comment for why it's a separate post-`start()` update rather than a `start()`
    /// parameter.
    pub allow_uncertain_quality: bool,
    /// Polling-cadence diagnostics collected from successful PV samples. `None` for runs
    /// created before timing diagnostics existed or attempts that ended before polling began.
    pub timing_metrics: Option<TimingMetrics>,
    /// Concrete timing values used by this run after configuration defaults were resolved.
    /// `None` for runs created before effective-tuning snapshots existed or until
    /// [`TuneRunRow::record_effective_tuning`] is called.
    pub effective_tuning: Option<EffectiveTuning>,
    /// Observed opcda-bridge compatibility report, stored as the driver's JSON snapshot.
    /// `None` for non-OPC runs, runs started before the snapshot existed, or until
    /// [`TuneRunRow::record_gateway_compatibility`] is called. Kept as raw JSON because the
    /// report type lives in `bhtune-driver`, which this crate does not depend on.
    pub gateway_compatibility_json: Option<String>,
    /// Outcome of the best-effort restore attempted after this run ended -- `None` if no
    /// restore was ever attempted (the run never mutated the loop, or hasn't ended yet). See
    /// [`RestoreStatus`] and [`TuneRunRow::record_restore_status`].
    pub restore_status: Option<RestoreStatus>,
    /// Set only alongside `restore_status = Some(RestoreStatus::Incomplete)`: what a second
    /// Ctrl+C, `[tuning].restore_timeout_secs`, or an individual failed restore step
    /// prevented from being confirmed.
    pub restore_detail: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Filter criteria for [`TuneRunRow::list`]/[`TuneRunRow::count`]. Every field is optional;
/// the all-`None` default matches every run. Build one with [`TuneRunFilter::default`] and
/// the `with_*` methods, e.g. `TuneRunFilter::default().with_outcome(TuneOutcome::Failed)`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TuneRunFilter {
    pub loop_id: Option<i64>,
    pub demo_session_id: Option<i64>,
    pub process_type: Option<ProcessType>,
    pub controller_type: Option<ControllerType>,
    pub outcome: Option<TuneOutcome>,
    pub driver: Option<TuneDriver>,
    /// Exact match against the stored `opc_server` column. See
    /// [`TuneRunRow::record_connection`].
    pub opc_server: Option<String>,
    /// Exact match against the stored `bridge_host` column. See
    /// [`TuneRunRow::record_connection`].
    pub bridge_host: Option<String>,
    /// Matches runs with `started_at >= started_after` (inclusive).
    pub started_after: Option<DateTime<Utc>>,
    /// Matches runs with `started_at <= started_before` (inclusive).
    pub started_before: Option<DateTime<Utc>>,
    pub template_name: Option<String>,
    pub template_origin: Option<TemplateOrigin>,
}

impl TuneRunFilter {
    pub fn with_demo_session_id(mut self, demo_session_id: i64) -> TuneRunFilter {
        self.demo_session_id = Some(demo_session_id);
        self
    }
    pub fn with_loop_id(mut self, loop_id: i64) -> TuneRunFilter {
        self.loop_id = Some(loop_id);
        self
    }

    pub fn with_process_type(mut self, process_type: ProcessType) -> TuneRunFilter {
        self.process_type = Some(process_type);
        self
    }

    pub fn with_controller_type(mut self, controller_type: ControllerType) -> TuneRunFilter {
        self.controller_type = Some(controller_type);
        self
    }

    pub fn with_outcome(mut self, outcome: TuneOutcome) -> TuneRunFilter {
        self.outcome = Some(outcome);
        self
    }

    pub fn with_driver(mut self, driver: TuneDriver) -> TuneRunFilter {
        self.driver = Some(driver);
        self
    }

    pub fn with_opc_server(mut self, opc_server: impl Into<String>) -> TuneRunFilter {
        self.opc_server = Some(opc_server.into());
        self
    }

    pub fn with_bridge_host(mut self, bridge_host: impl Into<String>) -> TuneRunFilter {
        self.bridge_host = Some(bridge_host.into());
        self
    }

    pub fn with_started_after(mut self, started_after: DateTime<Utc>) -> TuneRunFilter {
        self.started_after = Some(started_after);
        self
    }

    pub fn with_started_before(mut self, started_before: DateTime<Utc>) -> TuneRunFilter {
        self.started_before = Some(started_before);
        self
    }

    pub fn with_template_name(mut self, template_name: impl Into<String>) -> TuneRunFilter {
        self.template_name = Some(template_name.into());
        self
    }

    pub fn with_template_origin(mut self, template_origin: TemplateOrigin) -> TuneRunFilter {
        self.template_origin = Some(template_origin);
        self
    }
}

/// A page of [`TuneRunRow::list`] results: `limit` rows starting at `offset`, ordered newest
/// first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pagination {
    pub limit: i64,
    pub offset: i64,
}

impl Pagination {
    pub fn new(limit: i64, offset: i64) -> Pagination {
        Pagination { limit, offset }
    }

    /// The first `limit` rows.
    pub fn first(limit: i64) -> Pagination {
        Pagination { limit, offset: 0 }
    }
}

impl Default for Pagination {
    /// 50 rows, offset 0 — a reasonable default page size for a CLI/GUI run list.
    fn default() -> Pagination {
        Pagination {
            limit: 50,
            offset: 0,
        }
    }
}

impl TuneRunRow {
    /// Starts a new run: inserts a `tune_runs` row with `outcome = 'running'` and no initial
    /// readings yet (see [`Self::record_initial_readings`]). `now` is used for both
    /// `started_at` and `created_at`, which are naturally the same instant for a run that's
    /// only just begun. `loop_id` may be `None` for an ad-hoc run against tags that were
    /// never saved as a reusable [`LoopRow`].
    ///
    /// `template_origin`/`template`/`tags` snapshot exactly what this run was configured
    /// against (`safety-run-snapshot`), so a historical run stays interpretable even after
    /// the template catalog changes underneath it. Serializing `template`/`tags` returns
    /// [`DbError::Serialize`] if the JSON encoder rejects a value. Both types are plain,
    /// `derive`d structures, and every `f32` field they can carry is validated finite well
    /// before a run reaches this call (see `safety-validation`); a serialization failure
    /// means that contract regressed upstream rather than a normal plant condition.
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        pool: &SqlitePool,
        loop_id: Option<i64>,
        loop_name: &str,
        driver: TuneDriver,
        config: LoopConfig,
        template_origin: TemplateOrigin,
        template: &DcsTemplate,
        tags: &LoopTags,
        now: DateTime<Utc>,
    ) -> DbResult<TuneRunRow> {
        Self::start_with_demo_session(
            pool,
            None,
            loop_id,
            loop_name,
            driver,
            config,
            template_origin,
            template,
            tags,
            now,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_with_demo_session(
        pool: &SqlitePool,
        demo_session_id: Option<i64>,
        loop_id: Option<i64>,
        loop_name: &str,
        driver: TuneDriver,
        config: LoopConfig,
        template_origin: TemplateOrigin,
        template: &DcsTemplate,
        tags: &LoopTags,
        now: DateTime<Utc>,
    ) -> DbResult<TuneRunRow> {
        let template_snapshot_json = json_text("template snapshot", template)?;
        let tags_json = json_text("loop tags", tags)?;

        let row = sqlx::query(
            r"
            INSERT INTO tune_runs (
                loop_id, demo_session_id, loop_name, driver, started_at, outcome,
                process_type, controller_type, relay_amp_percent, num_cycles_skip,
                num_cycles_count, noise_protection_secs, mrft_delay_secs,
                template_name, template_origin, template_snapshot_json, tags_json,
                created_at
            )
            SELECT ?, ?, ?, ?, ?, 'running', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
            WHERE ? IS NULL OR EXISTS (
                SELECT 1
                FROM demo_sessions
                WHERE id = ? AND revoked_at IS NULL AND expires_at > ?
            )
            RETURNING *
            ",
        )
        .bind(loop_id)
        .bind(demo_session_id)
        .bind(loop_name)
        .bind(enum_to_text(&driver)?)
        .bind(now)
        .bind(enum_to_text(&config.process_type)?)
        .bind(enum_to_text(&config.controller_type)?)
        .bind(config.relay_amp_percent)
        .bind(config.num_cycles_skip)
        .bind(config.num_cycles_count)
        .bind(config.noise_protection_secs)
        .bind(config.mrft_delay_secs)
        .bind(&template.name)
        .bind(enum_to_text(&template_origin)?)
        .bind(template_snapshot_json)
        .bind(tags_json)
        .bind(now)
        .bind(demo_session_id)
        .bind(demo_session_id)
        .bind(now)
        .fetch_optional(pool)
        .await
        .map_err(DbError::Query)?
        .ok_or_else(|| DbError::Query(sqlx::Error::RowNotFound))?;

        row_to_tune_run(row)
    }

    /// Starts a simulator run owned by a demo session. The database trigger rejects any
    /// attempt to attach an owner to a live OPC DA run.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_owned(
        pool: &SqlitePool,
        demo_session_id: i64,
        loop_name: &str,
        config: LoopConfig,
        template_origin: TemplateOrigin,
        template: &DcsTemplate,
        tags: &LoopTags,
        now: DateTime<Utc>,
    ) -> DbResult<TuneRunRow> {
        Self::start_with_demo_session(
            pool,
            Some(demo_session_id),
            None,
            loop_name,
            TuneDriver::Simulator,
            config,
            template_origin,
            template,
            tags,
            now,
        )
        .await
    }

    pub async fn get_for_demo_session(
        pool: &SqlitePool,
        run_id: i64,
        demo_session_id: i64,
    ) -> DbResult<Option<TuneRunRow>> {
        let row = sqlx::query("SELECT * FROM tune_runs WHERE id = ? AND demo_session_id = ?")
            .bind(run_id)
            .bind(demo_session_id)
            .fetch_optional(pool)
            .await
            .map_err(DbError::Query)?;
        row.map(row_to_tune_run).transpose()
    }

    pub async fn list_for_demo_session(
        pool: &SqlitePool,
        demo_session_id: i64,
        pagination: Pagination,
    ) -> DbResult<Vec<TuneRunRow>> {
        let rows = sqlx::query(
            "SELECT * FROM tune_runs WHERE demo_session_id = ? \
             ORDER BY started_at DESC, id DESC LIMIT ? OFFSET ?",
        )
        .bind(demo_session_id)
        .bind(pagination.limit)
        .bind(pagination.offset)
        .fetch_all(pool)
        .await
        .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_tune_run).collect()
    }

    pub async fn newest_for_demo_session(
        pool: &SqlitePool,
        demo_session_id: i64,
    ) -> DbResult<Option<TuneRunRow>> {
        let row = sqlx::query(
            "SELECT * FROM tune_runs WHERE demo_session_id = ? \
             ORDER BY started_at DESC, id DESC LIMIT 1",
        )
        .bind(demo_session_id)
        .fetch_optional(pool)
        .await
        .map_err(DbError::Query)?;
        row.map(row_to_tune_run).transpose()
    }

    pub async fn count_for_demo_session(pool: &SqlitePool, demo_session_id: i64) -> DbResult<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM tune_runs WHERE demo_session_id = ?")
            .bind(demo_session_id)
            .fetch_one(pool)
            .await
            .map_err(DbError::Query)
    }

    /// Counts every current Demo-owned run across all sessions and outcomes. Full-mode rows have
    /// `demo_session_id = NULL` and are excluded.
    pub async fn count_demo_owned(pool: &SqlitePool) -> DbResult<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM tune_runs WHERE demo_session_id IS NOT NULL")
            .fetch_one(pool)
            .await
            .map_err(DbError::Query)
    }

    /// Deletes terminal history beyond the newest `retain` rows for one Demo owner. Running
    /// rows never consume a retention slot and are never deleted. Child samples, results,
    /// writes, and MV-actuation evidence cascade with each deleted run.
    pub async fn prune_terminal_for_demo_session(
        pool: &SqlitePool,
        demo_session_id: i64,
        retain: u32,
    ) -> DbResult<u64> {
        let result = sqlx::query(
            "DELETE FROM tune_runs \
             WHERE id IN ( \
                 SELECT id FROM tune_runs \
                 WHERE demo_session_id = ? AND outcome <> 'running' \
                 ORDER BY started_at DESC, id DESC \
                 LIMIT -1 OFFSET ? \
             )",
        )
        .bind(demo_session_id)
        .bind(i64::from(retain))
        .execute(pool)
        .await
        .map_err(DbError::Query)?;
        Ok(result.rows_affected())
    }

    /// Applies [`Self::prune_terminal_for_demo_session`]'s retention rule independently to
    /// every Demo owner in one statement. Intended for periodic global cleanup.
    pub async fn prune_terminal_demo_owned(pool: &SqlitePool, retain: u32) -> DbResult<u64> {
        let result = sqlx::query(
            "WITH ranked AS ( \
                 SELECT id, ROW_NUMBER() OVER ( \
                     PARTITION BY demo_session_id ORDER BY started_at DESC, id DESC \
                 ) AS retention_rank \
                 FROM tune_runs \
                 WHERE demo_session_id IS NOT NULL AND outcome <> 'running' \
             ) \
             DELETE FROM tune_runs \
             WHERE id IN (SELECT id FROM ranked WHERE retention_rank > ?)",
        )
        .bind(i64::from(retain))
        .execute(pool)
        .await
        .map_err(DbError::Query)?;
        Ok(result.rows_affected())
    }

    pub async fn count_rows_for_demo_session(
        pool: &SqlitePool,
        demo_session_id: i64,
    ) -> DbResult<i64> {
        sqlx::query_scalar(
            "SELECT
                (SELECT COUNT(*) FROM tune_runs WHERE demo_session_id = ?)
              + (SELECT COUNT(*) FROM tune_samples WHERE run_id IN
                    (SELECT id FROM tune_runs WHERE demo_session_id = ?))
              + (SELECT COUNT(*) FROM tune_results WHERE run_id IN
                    (SELECT id FROM tune_runs WHERE demo_session_id = ?))
              + (SELECT COUNT(*) FROM tune_writes WHERE run_id IN
                    (SELECT id FROM tune_runs WHERE demo_session_id = ?))
              + (SELECT COUNT(*) FROM tune_mv_actuations WHERE run_id IN
                    (SELECT id FROM tune_runs WHERE demo_session_id = ?))",
        )
        .bind(demo_session_id)
        .bind(demo_session_id)
        .bind(demo_session_id)
        .bind(demo_session_id)
        .bind(demo_session_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)
    }

    /// Records this run's connection provenance and the exact request it was started with
    /// (`db-run-request-snapshot`): the OPC DA server ProgID and opcda-bridge gateway host
    /// actually used (`None`/`None` for a non-opcda run), and a JSON snapshot of the complete
    /// submitted request (CLI flags or the HTTP `POST /api/runs` body), captured *before* any
    /// config-driven defaulting so it reflects what the caller actually asked for.
    ///
    /// A separate post-`start()` update rather than three more `start()` parameters, matching
    /// [`Self::record_allow_uncertain_quality`]'s precedent -- `start()` already has 8
    /// positional parameters across dozens of call sites in this workspace's test suites
    /// alone, and three more would make every one of them noisier for no benefit, since none
    /// of those tests care about connection provenance. Unlike that method, this data *is*
    /// normally known the instant a run begins; the one production caller (the `bhtune` CLI's
    /// `prepare()`) calls this immediately after `start()` succeeds, before any driver I/O.
    /// `opc_server`/`bridge_host` default to `NULL` and `request_json` defaults to `"{}"`
    /// (see the migration), so every existing `start()` call site keeps compiling and
    /// behaving exactly as before.
    pub async fn record_connection(
        pool: &SqlitePool,
        run_id: i64,
        opc_server: Option<&str>,
        bridge_host: Option<&str>,
        request_json: &str,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            r"
            UPDATE tune_runs SET opc_server = ?, bridge_host = ?, request_json = ?
            WHERE id = ?
            RETURNING *
            ",
        )
        .bind(opc_server)
        .bind(bridge_host)
        .bind(request_json)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Records the concrete tune timing policy after all configuration defaults have been
    /// resolved. This is a follow-up update rather than another [`Self::start`] parameter so
    /// existing repository callers remain source-compatible. Production orchestration should
    /// call it immediately after `start()` and before any driver I/O.
    pub async fn record_effective_tuning(
        pool: &SqlitePool,
        run_id: i64,
        effective_tuning: EffectiveTuning,
    ) -> DbResult<TuneRunRow> {
        let effective_tuning_json = json_text("effective tuning", &effective_tuning)?;
        let row = sqlx::query(
            r"
            UPDATE tune_runs SET effective_tuning_json = ?
            WHERE id = ?
            RETURNING *
            ",
        )
        .bind(effective_tuning_json)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Records the observed opcda-bridge compatibility snapshot for a live OPC DA run.
    ///
    /// `gateway_compatibility_json` must already be valid JSON. This is a follow-up update
    /// rather than another [`Self::start`] parameter so existing repository callers remain
    /// source-compatible.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Query`] when the row does not exist or the JSON fails the column
    /// check.
    pub async fn record_gateway_compatibility(
        pool: &SqlitePool,
        run_id: i64,
        gateway_compatibility_json: &str,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            r"
            UPDATE tune_runs SET gateway_compatibility_json = ?
            WHERE id = ?
            RETURNING *
            ",
        )
        .bind(gateway_compatibility_json)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Replaces this run's operator notes. Passing `None` clears the note, which is the
    /// persistence-layer implementation of the GUI's delete-note action. This deliberately
    /// has no lifecycle restriction: notes remain editable while a run is active and after it
    /// reaches a terminal outcome.
    pub async fn update_notes(
        pool: &SqlitePool,
        run_id: i64,
        notes: Option<&str>,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            r"
            UPDATE tune_runs SET notes = ?
            WHERE id = ?
            RETURNING *
            ",
        )
        .bind(notes)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Records the driver's initial-readings snapshot (`ReadInitialOPCvalues` in the legacy
    /// app) for an already-started run. Called at most once per run, right after that read
    /// succeeds -- and, deliberately, *before* `transition_to_manual`'s first mutating write
    /// rather than after it (`safety-restore-guard`, finding 3 of the live-plant safety
    /// review), so `mode_raw`/`mode_attribute_raw`/`setpoint_ini` are always durably
    /// persisted before the loop is touched at all, letting a crashed run be reconstructed
    /// and restored later via `bhtune restore-loop`. A run that fails before or during the
    /// read instead goes straight to [`Self::fail`] with `initial_readings` left `None`.
    pub async fn record_initial_readings(
        pool: &SqlitePool,
        run_id: i64,
        readings: TuneRunInitialReadings,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            r"
            UPDATE tune_runs SET
                pv_ini = ?, mv_ini = ?, mv_range_low = ?, mv_range_high = ?,
                pv_range_high = ?, pv_range_low = ?, controller_direction = ?,
                mode_raw = ?, mode_attribute_raw = ?, setpoint_ini = ?
            WHERE id = ?
            RETURNING *
            ",
        )
        .bind(readings.pv_ini)
        .bind(readings.mv_ini)
        .bind(readings.mv_range_low)
        .bind(readings.mv_range_high)
        .bind(readings.pv_range_high)
        .bind(readings.pv_range_low)
        .bind(enum_to_text(&readings.controller_direction)?)
        .bind(readings.mode_raw)
        .bind(readings.mode_attribute_raw)
        .bind(readings.setpoint_ini)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Records whether this run permitted `Quality::Uncertain` OPC readings under the global
    /// configuration policy (finding 5 of the live-plant safety review). A separate
    /// post-`start()` update rather than a new `start()` parameter deliberately: `start()`
    /// already has 8 positional parameters across 28 call sites in this crate's own test
    /// suite alone, and this is a rarely-used escape hatch, not information every caller
    /// naturally has on hand at the moment a run begins the way `template_origin`/`template`/
    /// `tags` are. The column defaults to `0`/`false` (see the migration), so every existing
    /// `start()` call site keeps compiling and behaving exactly as before; only the one
    /// production caller in the `bhtune` package's `run()` needs to call this, right after `start()`
    /// succeeds.
    pub async fn record_allow_uncertain_quality(
        pool: &SqlitePool,
        run_id: i64,
        allow_uncertain_quality: bool,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            r"
            UPDATE tune_runs SET allow_uncertain_quality = ?
            WHERE id = ?
            RETURNING *
            ",
        )
        .bind(allow_uncertain_quality)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Records the polling cadence observed while this run was active. Kept as one typed JSON
    /// snapshot because these diagnostics are nested, evolve together, and have no SQL-level
    /// filtering requirement. Normal completed/aborted tune orchestration uses
    /// [`Self::complete_with_timing_metrics`] or [`Self::abort_with_timing_metrics`] so the
    /// terminal outcome and diagnostics become visible atomically; this standalone update is
    /// retained for non-terminal/failure paths and direct repository consumers.
    pub async fn record_timing_metrics(
        pool: &SqlitePool,
        run_id: i64,
        metrics: TimingMetrics,
    ) -> DbResult<TuneRunRow> {
        let metrics_json = json_text("timing metrics", &metrics)?;
        let row = sqlx::query(
            r"
            UPDATE tune_runs SET timing_metrics_json = ?
            WHERE id = ?
            RETURNING *
            ",
        )
        .bind(metrics_json)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Records the outcome of a best-effort loop-restore attempt made after this run ended
    /// (`safety-restore-guard`, finding 3 of the live-plant safety review). Called once,
    /// after `complete`/`fail`/`abort` (whichever applies) and after `attempt_restore` has
    /// actually run -- never before, and never for a run that ended without ever mutating
    /// the loop (nothing to restore, so nothing to record). `detail` should be `Some(..)`
    /// whenever `status` is [`RestoreStatus::Incomplete`], naming what could not be
    /// confirmed; pass `None` for [`RestoreStatus::Confirmed`]. A separate post-hoc update
    /// rather than a `complete`/`fail`/`abort` parameter, matching
    /// [`Self::record_allow_uncertain_quality`]'s precedent: the restore attempt always
    /// happens strictly after one of those three, never alongside it.
    pub async fn record_restore_status(
        pool: &SqlitePool,
        run_id: i64,
        status: RestoreStatus,
        detail: Option<&str>,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            r"
            UPDATE tune_runs SET restore_status = ?, restore_detail = ?
            WHERE id = ?
            RETURNING *
            ",
        )
        .bind(enum_to_text(&status)?)
        .bind(detail)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Marks a run `completed` — a full MRFT test that ran to its natural end. The calculated
    /// results themselves are recorded separately via [`TuneResultRow::insert`].
    pub async fn complete(
        pool: &SqlitePool,
        run_id: i64,
        completed_at: DateTime<Utc>,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            "UPDATE tune_runs SET outcome = 'completed', completed_at = ? WHERE id = ? RETURNING *",
        )
        .bind(completed_at)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Atomically marks a run completed and publishes its timing diagnostics. This prevents
    /// readers that react to the terminal outcome (notably the SSE stream) from observing a
    /// completed run before its timing snapshot is visible.
    pub async fn complete_with_timing_metrics(
        pool: &SqlitePool,
        run_id: i64,
        completed_at: DateTime<Utc>,
        timing_metrics: Option<TimingMetrics>,
    ) -> DbResult<TuneRunRow> {
        Self::set_terminal_outcome_with_timing_metrics(
            pool,
            run_id,
            completed_at,
            TuneOutcome::Completed,
            timing_metrics,
            None,
        )
        .await
    }

    /// Marks a run `failed`, recording why. Valid whether or not
    /// [`Self::record_initial_readings`] was ever called for this run — a run can fail before,
    /// during, or after the initial read.
    pub async fn fail(
        pool: &SqlitePool,
        run_id: i64,
        completed_at: DateTime<Utc>,
        failure_reason: &str,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            r"
            UPDATE tune_runs SET outcome = 'failed', completed_at = ?, failure_reason = ?
            WHERE id = ?
            RETURNING *
            ",
        )
        .bind(completed_at)
        .bind(failure_reason)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Marks a run `aborted` — stopped deliberately (by a human, or `cli-safety`'s
    /// wall-clock timeout guardrail) rather than failing on its own.
    pub async fn abort(
        pool: &SqlitePool,
        run_id: i64,
        completed_at: DateTime<Utc>,
    ) -> DbResult<TuneRunRow> {
        let row = sqlx::query(
            "UPDATE tune_runs SET outcome = 'aborted', completed_at = ? WHERE id = ? RETURNING *",
        )
        .bind(completed_at)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Atomically marks a run aborted and publishes any timing diagnostics collected before
    /// the abort, so terminal-state readers cannot miss the final timing snapshot.
    pub async fn abort_with_timing_metrics(
        pool: &SqlitePool,
        run_id: i64,
        completed_at: DateTime<Utc>,
        timing_metrics: Option<TimingMetrics>,
    ) -> DbResult<TuneRunRow> {
        Self::set_terminal_outcome_with_timing_metrics(
            pool,
            run_id,
            completed_at,
            TuneOutcome::Aborted,
            timing_metrics,
            None,
        )
        .await
    }

    /// Atomically marks a run aborted, publishes its timing diagnostics, and persists the
    /// operator-facing reason for the abort in the existing `failure_reason` column. This is
    /// used when an abort has a durable safety explanation (for example an MV command whose
    /// live readback did not reach its target), while preserving the database's existing
    /// [`TuneOutcome::Aborted`] value.
    pub async fn abort_with_timing_metrics_and_reason(
        pool: &SqlitePool,
        run_id: i64,
        completed_at: DateTime<Utc>,
        timing_metrics: Option<TimingMetrics>,
        reason: &str,
    ) -> DbResult<TuneRunRow> {
        Self::set_terminal_outcome_with_timing_metrics(
            pool,
            run_id,
            completed_at,
            TuneOutcome::Aborted,
            timing_metrics,
            Some(reason),
        )
        .await
    }

    async fn set_terminal_outcome_with_timing_metrics(
        pool: &SqlitePool,
        run_id: i64,
        completed_at: DateTime<Utc>,
        outcome: TuneOutcome,
        timing_metrics: Option<TimingMetrics>,
        failure_reason: Option<&str>,
    ) -> DbResult<TuneRunRow> {
        let timing_metrics_json = timing_metrics
            .map(|metrics| json_text("timing metrics", &metrics))
            .transpose()?;
        let row = sqlx::query(
            r"
            UPDATE tune_runs
            SET outcome = ?, completed_at = ?, timing_metrics_json = ?, failure_reason = ?
            WHERE id = ?
            RETURNING *
            ",
        )
        .bind(enum_to_text(&outcome)?)
        .bind(completed_at)
        .bind(timing_metrics_json)
        .bind(failure_reason)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_run(row)
    }

    /// Fetches one row by id, or `None` if it doesn't exist.
    pub async fn get(pool: &SqlitePool, id: i64) -> DbResult<Option<TuneRunRow>> {
        let row = sqlx::query("SELECT * FROM tune_runs WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(DbError::Query)?;
        row.map(row_to_tune_run).transpose()
    }

    /// Lists runs matching `filter`, newest-started first, one `pagination` page at a time.
    /// See [`Self::count`] for the total number of rows `filter` matches across all pages.
    pub async fn list(
        pool: &SqlitePool,
        filter: &TuneRunFilter,
        pagination: Pagination,
    ) -> DbResult<Vec<TuneRunRow>> {
        let mut builder: QueryBuilder<Sqlite> = QueryBuilder::new("SELECT * FROM tune_runs");
        push_filter(&mut builder, filter)?;
        builder.push(" ORDER BY started_at DESC LIMIT ");
        builder.push_bind(pagination.limit);
        builder.push(" OFFSET ");
        builder.push_bind(pagination.offset);

        let rows = builder
            .build()
            .fetch_all(pool)
            .await
            .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_tune_run).collect()
    }

    /// Counts every run matching `filter`, ignoring pagination — the total [`Self::list`]
    /// would page through.
    pub async fn count(pool: &SqlitePool, filter: &TuneRunFilter) -> DbResult<i64> {
        let mut builder: QueryBuilder<Sqlite> = QueryBuilder::new("SELECT COUNT(*) FROM tune_runs");
        push_filter(&mut builder, filter)?;
        builder
            .build_query_scalar::<i64>()
            .fetch_one(pool)
            .await
            .map_err(DbError::Query)
    }

    /// Deletes every run matching `filter` in one statement (SQLite treats a single
    /// statement as its own transaction, so no explicit `BEGIN`/`COMMIT` is needed). Returns
    /// the number of runs deleted. `tune_samples`/`tune_results`/`tune_writes`'s `ON DELETE
    /// CASCADE` foreign keys (see `db-schema`'s migration) remove each deleted run's samples,
    /// results, and write-back audit rows automatically.
    ///
    /// Shares [`push_filter`] with [`Self::list`]/[`Self::count`], so "what a `--dry-run`
    /// preview reports" and "what an actual sweep deletes" can never disagree — used this way
    /// by `history-retention`'s automatic sweep and `bhtune history prune`.
    ///
    /// An empty `filter` (every field `None`) matches and deletes every run in the table —
    /// callers that mean to scope a deletion must build a `filter` that says so explicitly;
    /// this function has no separate "are you sure" guard of its own, matching `count`/`list`
    /// treating an empty filter as "everything" rather than "nothing".
    pub async fn delete_matching(pool: &SqlitePool, filter: &TuneRunFilter) -> DbResult<u64> {
        let mut builder: QueryBuilder<Sqlite> = QueryBuilder::new("DELETE FROM tune_runs");
        push_filter(&mut builder, filter)?;
        let result = builder
            .build()
            .execute(pool)
            .await
            .map_err(DbError::Query)?;
        Ok(result.rows_affected())
    }

    /// Deletes exactly one run by id (`history-explorer-ui`'s delete action). Returns
    /// whether a row was actually deleted -- `false` if no run has that id, letting the
    /// caller map that to a 404 rather than a silent no-op. Unlike
    /// [`DcsTemplateRow::delete`], no foreign key ever blocks this: `tune_runs` has no
    /// parent-side `RESTRICT` reference pointing at it, only the `ON DELETE CASCADE`
    /// children (`tune_samples`/`tune_results`/`tune_writes`, see `db-schema`'s migration),
    /// which SQLite removes automatically as part of the same statement.
    pub async fn delete(pool: &SqlitePool, id: i64) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM tune_runs WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await
            .map_err(DbError::Query)?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn delete_for_demo_session(
        pool: &SqlitePool,
        id: i64,
        demo_session_id: i64,
    ) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM tune_runs WHERE id = ? AND demo_session_id = ?")
            .bind(id)
            .bind(demo_session_id)
            .execute(pool)
            .await
            .map_err(DbError::Query)?;
        Ok(result.rows_affected() > 0)
    }
}

/// Appends `WHERE <conditions>` to `builder` for every `Some` field in `filter`, or nothing
/// at all if every field is `None`. Shared by [`TuneRunRow::list`]/[`TuneRunRow::count`] so
/// the two can never disagree about which rows match a given filter. Returns
/// [`DbError::Serialize`] if an enum filter value cannot be encoded.
fn push_filter(builder: &mut QueryBuilder<Sqlite>, filter: &TuneRunFilter) -> DbResult<()> {
    // `1=1` makes every real condition an unconditional `AND`, rather than needing to track
    // whether it's the first one (and therefore needs `WHERE` instead of `AND`).
    builder.push(" WHERE 1=1");

    if let Some(loop_id) = filter.loop_id {
        builder.push(" AND loop_id = ").push_bind(loop_id);
    }
    if let Some(demo_session_id) = filter.demo_session_id {
        builder
            .push(" AND demo_session_id = ")
            .push_bind(demo_session_id);
    }
    if let Some(process_type) = filter.process_type {
        builder
            .push(" AND process_type = ")
            .push_bind(enum_to_text(&process_type)?);
    }
    if let Some(controller_type) = filter.controller_type {
        builder
            .push(" AND controller_type = ")
            .push_bind(enum_to_text(&controller_type)?);
    }
    if let Some(outcome) = filter.outcome {
        builder
            .push(" AND outcome = ")
            .push_bind(enum_to_text(&outcome)?);
    }
    if let Some(driver) = filter.driver {
        builder
            .push(" AND driver = ")
            .push_bind(enum_to_text(&driver)?);
    }
    if let Some(opc_server) = &filter.opc_server {
        builder
            .push(" AND opc_server = ")
            .push_bind(opc_server.clone());
    }
    if let Some(bridge_host) = &filter.bridge_host {
        builder
            .push(" AND bridge_host = ")
            .push_bind(bridge_host.clone());
    }
    if let Some(started_after) = filter.started_after {
        builder.push(" AND started_at >= ").push_bind(started_after);
    }
    if let Some(started_before) = filter.started_before {
        builder
            .push(" AND started_at <= ")
            .push_bind(started_before);
    }
    if let Some(template_name) = &filter.template_name {
        builder
            .push(" AND template_name = ")
            .push_bind(template_name.clone());
    }
    if let Some(template_origin) = filter.template_origin {
        builder
            .push(" AND template_origin = ")
            .push_bind(enum_to_text(&template_origin)?);
    }
    Ok(())
}

fn row_to_tune_run(row: SqliteRow) -> DbResult<TuneRunRow> {
    let pv_ini: Option<f32> = row.try_get("pv_ini").map_err(DbError::Query)?;
    let initial_readings = match pv_ini {
        Some(pv_ini) => {
            let controller_direction: String = row
                .try_get("controller_direction")
                .map_err(DbError::Query)?;
            Some(TuneRunInitialReadings {
                pv_ini,
                mv_ini: row.try_get("mv_ini").map_err(DbError::Query)?,
                mv_range_low: row.try_get("mv_range_low").map_err(DbError::Query)?,
                mv_range_high: row.try_get("mv_range_high").map_err(DbError::Query)?,
                pv_range_high: row.try_get("pv_range_high").map_err(DbError::Query)?,
                pv_range_low: row.try_get("pv_range_low").map_err(DbError::Query)?,
                controller_direction: text_to_enum("controller_direction", &controller_direction)?,
                mode_raw: row.try_get("mode_raw").map_err(DbError::Query)?,
                mode_attribute_raw: row.try_get("mode_attribute_raw").map_err(DbError::Query)?,
                setpoint_ini: row.try_get("setpoint_ini").map_err(DbError::Query)?,
            })
        }
        None => None,
    };

    let process_type: String = row.try_get("process_type").map_err(DbError::Query)?;
    let controller_type: String = row.try_get("controller_type").map_err(DbError::Query)?;
    let config = LoopConfig {
        process_type: text_to_enum("process_type", &process_type)?,
        controller_type: text_to_enum("controller_type", &controller_type)?,
        relay_amp_percent: row.try_get("relay_amp_percent").map_err(DbError::Query)?,
        num_cycles_skip: row.try_get("num_cycles_skip").map_err(DbError::Query)?,
        num_cycles_count: row.try_get("num_cycles_count").map_err(DbError::Query)?,
        noise_protection_secs: row
            .try_get("noise_protection_secs")
            .map_err(DbError::Query)?,
        mrft_delay_secs: row.try_get("mrft_delay_secs").map_err(DbError::Query)?,
    };

    let driver: String = row.try_get("driver").map_err(DbError::Query)?;
    let outcome: String = row.try_get("outcome").map_err(DbError::Query)?;

    let restore_status_text: Option<String> =
        row.try_get("restore_status").map_err(DbError::Query)?;
    let restore_status = restore_status_text
        .map(|text| text_to_enum("restore_status", &text))
        .transpose()?;

    let template_origin: String = row.try_get("template_origin").map_err(DbError::Query)?;
    let template_snapshot_json: String = row
        .try_get("template_snapshot_json")
        .map_err(DbError::Query)?;
    let tags_json: String = row.try_get("tags_json").map_err(DbError::Query)?;
    let request_json: String = row.try_get("request_json").map_err(DbError::Query)?;
    let timing_metrics_json: Option<String> =
        row.try_get("timing_metrics_json").map_err(DbError::Query)?;
    let effective_tuning_json: Option<String> = row
        .try_get("effective_tuning_json")
        .map_err(DbError::Query)?;
    let gateway_compatibility_json: Option<String> = row
        .try_get("gateway_compatibility_json")
        .map_err(DbError::Query)?;
    let template: DcsTemplate =
        serde_json::from_str(&template_snapshot_json).map_err(|source| {
            DbError::InvalidJsonShape {
                column: "template_snapshot_json",
                source,
            }
        })?;
    let tags: LoopTags =
        serde_json::from_str(&tags_json).map_err(|source| DbError::InvalidJsonShape {
            column: "tags_json",
            source,
        })?;
    let timing_metrics = timing_metrics_json
        .map(|json| {
            serde_json::from_str(&json).map_err(|source| DbError::InvalidJsonShape {
                column: "timing_metrics_json",
                source,
            })
        })
        .transpose()?;
    let effective_tuning = effective_tuning_json
        .map(|json| {
            serde_json::from_str(&json).map_err(|source| DbError::InvalidJsonShape {
                column: "effective_tuning_json",
                source,
            })
        })
        .transpose()?;

    Ok(TuneRunRow {
        id: row.try_get("id").map_err(DbError::Query)?,
        loop_id: row.try_get("loop_id").map_err(DbError::Query)?,
        demo_session_id: row.try_get("demo_session_id").map_err(DbError::Query)?,
        loop_name: row.try_get("loop_name").map_err(DbError::Query)?,
        driver: text_to_enum("driver", &driver)?,
        opc_server: row.try_get("opc_server").map_err(DbError::Query)?,
        bridge_host: row.try_get("bridge_host").map_err(DbError::Query)?,
        started_at: row.try_get("started_at").map_err(DbError::Query)?,
        completed_at: row.try_get("completed_at").map_err(DbError::Query)?,
        outcome: text_to_enum("outcome", &outcome)?,
        failure_reason: row.try_get("failure_reason").map_err(DbError::Query)?,
        config,
        template_origin: text_to_enum("template_origin", &template_origin)?,
        template,
        tags,
        request_json,
        notes: row.try_get("notes").map_err(DbError::Query)?,
        initial_readings,
        allow_uncertain_quality: row
            .try_get("allow_uncertain_quality")
            .map_err(DbError::Query)?,
        timing_metrics,
        effective_tuning,
        gateway_compatibility_json,
        restore_status,
        restore_detail: row.try_get("restore_detail").map_err(DbError::Query)?,
        created_at: row.try_get("created_at").map_err(DbError::Query)?,
    })
}
// }}}1

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::{enum_to_text, text_to_enum};

    async fn sample_run() -> (crate::SqlitePool, i64) {
        let pool = crate::connect_in_memory().await.unwrap();
        let template = bhtune_core::built_in_templates().remove(0);
        let tags = bhtune_core::LoopTags::derive_from_pv_tag("Unit1.FIC101.PV", &template);
        let config = bhtune_core::LoopConfig {
            process_type: bhtune_core::ProcessType::Flow,
            controller_type: bhtune_core::ControllerType::Pi,
            relay_amp_percent: 5.0,
            num_cycles_skip: 1,
            num_cycles_count: 2,
            noise_protection_secs: 3,
            mrft_delay_secs: 0,
        };
        let run = TuneRunRow::start(
            &pool,
            None,
            "Unit1.FIC101.PV",
            TuneDriver::Simulator,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        (pool, run.id)
    }

    #[test]
    fn tune_driver_round_trips_and_matches_check_constraint() {
        let cases = [
            (TuneDriver::Opcda, "opcda"),
            (TuneDriver::Simulator, "simulator"),
            (TuneDriver::Replay, "replay"),
        ];
        for (variant, text) in cases {
            assert_eq!(enum_to_text(&variant).unwrap(), text);
            assert_eq!(text_to_enum::<TuneDriver>("driver", text).unwrap(), variant);
        }
    }

    #[test]
    fn tune_outcome_round_trips_and_matches_check_constraint() {
        let cases = [
            (TuneOutcome::Running, "running"),
            (TuneOutcome::Completed, "completed"),
            (TuneOutcome::Failed, "failed"),
            (TuneOutcome::Aborted, "aborted"),
        ];
        for (variant, text) in cases {
            assert_eq!(enum_to_text(&variant).unwrap(), text);
            assert_eq!(
                text_to_enum::<TuneOutcome>("outcome", text).unwrap(),
                variant
            );
        }
    }

    #[tokio::test]
    async fn record_restore_status_round_trips_both_status_shapes() {
        let (pool, run_id) = sample_run().await;
        let confirmed =
            TuneRunRow::record_restore_status(&pool, run_id, RestoreStatus::Confirmed, None)
                .await
                .unwrap();
        assert_eq!(confirmed.restore_status, Some(RestoreStatus::Confirmed));
        assert_eq!(confirmed.restore_detail, None);

        let incomplete = TuneRunRow::record_restore_status(
            &pool,
            run_id,
            RestoreStatus::Incomplete,
            Some("MV restore failed"),
        )
        .await
        .unwrap();
        assert_eq!(incomplete.restore_status, Some(RestoreStatus::Incomplete));
        assert_eq!(
            incomplete.restore_detail.as_deref(),
            Some("MV restore failed")
        );
    }

    #[tokio::test]
    async fn tune_run_delete_reports_both_existing_and_missing_ids() {
        let (pool, run_id) = sample_run().await;
        assert!(TuneRunRow::delete(&pool, run_id).await.unwrap());
        assert!(!TuneRunRow::delete(&pool, run_id).await.unwrap());
    }

    #[tokio::test]
    async fn malformed_tune_run_json_is_reported_with_the_respective_column() {
        for (column, expected) in [
            ("template_snapshot_json", "template_snapshot_json"),
            ("tags_json", "tags_json"),
            ("timing_metrics_json", "timing_metrics_json"),
            ("effective_tuning_json", "effective_tuning_json"),
        ] {
            let (pool, run_id) = sample_run().await;
            let query = match column {
                "template_snapshot_json" => {
                    sqlx::query("UPDATE tune_runs SET template_snapshot_json = ? WHERE id = ?")
                }
                "tags_json" => sqlx::query("UPDATE tune_runs SET tags_json = ? WHERE id = ?"),
                "timing_metrics_json" => {
                    sqlx::query("UPDATE tune_runs SET timing_metrics_json = ? WHERE id = ?")
                }
                "effective_tuning_json" => {
                    sqlx::query("UPDATE tune_runs SET effective_tuning_json = ? WHERE id = ?")
                }
                _ => unreachable!(),
            };
            query
                .bind("\"wrong-shape\"")
                .bind(run_id)
                .execute(&pool)
                .await
                .unwrap();
            let err = TuneRunRow::get(&pool, run_id).await.unwrap_err();
            assert!(matches!(
                err,
                DbError::InvalidJsonShape { column: actual, .. } if actual == expected
            ));
        }
    }
}
