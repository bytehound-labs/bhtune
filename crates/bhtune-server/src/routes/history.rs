//! Read-only run-history routes: `GET /api/runs` (filtered, paginated list) and
//! `GET /api/runs/{id}` (full run detail: config, initial readings, samples, results,
//! writes).
//!
//! DTO shapes deliberately mirror `bhtune-cli`'s `commands::history` `--output json` JSON
//! (`RunSummaryJson`/`RunDetailJson`/etc.) field-for-field, so the CLI and the HTTP API
//! describe the same run the same way -- one shape for the product's two faces, per this
//! workspace's DTO-decoupling convention (every JSON-facing consumer builds its own
//! projection of the non-`Serialize` `bhtune-db` row types, rather than the row types
//! themselves growing a `Serialize` impl). The one deliberate addition over the CLI's own
//! `RunDetailJson` is a full `samples` array (not just a `samples_recorded` count) -- the
//! future trend chart (`frontend-screens`/`history-explorer-ui`) needs the raw per-tick data,
//! and the data-volume math in AGENTS.md's "History explorer" section (thousands of rows per
//! run, not millions) says inlining it is cheap enough not to need its own paginated route.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use bhtune_core::{
    ControllerDirection, ControllerType, LoopConfig, ProcessType, ResponseLevel, Tick,
};
use bhtune_db::models::{
    Pagination, RestoreStatus, RollbackState, SampleQuality, TemplateOrigin, TuneBackend,
    TuneOutcome, TuneResultRow, TuneRunFilter, TuneRunRow, TuneSampleRow, TuneWriteRow, WriteKind,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

/// Query parameters for `GET /api/runs`, mirroring [`TuneRunFilter`]'s fields one-to-one
/// plus [`Pagination`]. Every field is optional; an absent `limit`/`offset` falls back to
/// [`Pagination::default`] (50 rows, offset 0), matching the CLI's own default page size.
#[derive(Debug, Deserialize)]
pub struct RunListQuery {
    pub loop_id: Option<i64>,
    pub process_type: Option<ProcessType>,
    pub controller_type: Option<ControllerType>,
    pub outcome: Option<TuneOutcome>,
    pub backend: Option<TuneBackend>,
    pub started_after: Option<DateTime<Utc>>,
    pub started_before: Option<DateTime<Utc>>,
    pub template_name: Option<String>,
    pub template_origin: Option<TemplateOrigin>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

fn filter_from_query(query: &RunListQuery) -> TuneRunFilter {
    let mut filter = TuneRunFilter::default();
    if let Some(v) = query.loop_id {
        filter = filter.with_loop_id(v);
    }
    if let Some(v) = query.process_type {
        filter = filter.with_process_type(v);
    }
    if let Some(v) = query.controller_type {
        filter = filter.with_controller_type(v);
    }
    if let Some(v) = query.outcome {
        filter = filter.with_outcome(v);
    }
    if let Some(v) = query.backend {
        filter = filter.with_backend(v);
    }
    if let Some(v) = query.started_after {
        filter = filter.with_started_after(v);
    }
    if let Some(v) = query.started_before {
        filter = filter.with_started_before(v);
    }
    if let Some(v) = &query.template_name {
        filter = filter.with_template_name(v.clone());
    }
    if let Some(v) = query.template_origin {
        filter = filter.with_template_origin(v);
    }
    filter
}

/// One run in `GET /api/runs`'s `runs` array -- deliberately a subset matching the CLI's own
/// `history list` table columns, not the full detail (that's [`RunDetailResponse`], for
/// `GET /api/runs/{id}`).
#[derive(Debug, Serialize)]
pub struct RunSummaryResponse {
    pub id: i64,
    pub loop_name: String,
    pub backend: TuneBackend,
    pub outcome: TuneOutcome,
    pub process_type: ProcessType,
    pub started_at: DateTime<Utc>,
}

impl From<&TuneRunRow> for RunSummaryResponse {
    fn from(run: &TuneRunRow) -> Self {
        RunSummaryResponse {
            id: run.id,
            loop_name: run.loop_name.clone(),
            backend: run.backend,
            outcome: run.outcome,
            process_type: run.config.process_type,
            started_at: run.started_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RunListResponse {
    pub runs: Vec<RunSummaryResponse>,
    /// How many rows are in `runs` (this page) -- distinct from `total`, the count of every
    /// run matching the filter across all pages.
    pub returned: usize,
    pub total: i64,
}

/// `GET /api/runs` -- newest-started-first, filtered by every present [`RunListQuery`] field,
/// one [`Pagination`] page at a time.
async fn list_runs(
    State(state): State<AppState>,
    Query(query): Query<RunListQuery>,
) -> Result<Json<RunListResponse>, ApiError> {
    let filter = filter_from_query(&query);
    let pagination = Pagination::new(
        query.limit.unwrap_or_else(|| Pagination::default().limit),
        query.offset.unwrap_or(0),
    );
    let runs = TuneRunRow::list(&state.pool, &filter, pagination).await?;
    let total = TuneRunRow::count(&state.pool, &filter).await?;
    Ok(Json(RunListResponse {
        returned: runs.len(),
        runs: runs.iter().map(RunSummaryResponse::from).collect(),
        total,
    }))
}

/// Local projection of [`bhtune_db::models::TuneRunInitialReadings`] -- see this module's
/// doc comment for why every JSON-facing type here is its own projection rather than a
/// `Serialize` impl on the `bhtune-db` row type.
#[derive(Debug, Serialize)]
pub struct InitialReadingsResponse {
    pub pv_ini: f32,
    pub mv_ini: f32,
    pub mv_range_low: f32,
    pub mv_range_high: f32,
    pub pv_range_high: f32,
    pub pv_range_low: f32,
    pub controller_direction: ControllerDirection,
    pub mode_raw: Option<String>,
    pub mode_attribute_raw: Option<String>,
    pub setpoint_ini: Option<f32>,
}

impl From<bhtune_db::models::TuneRunInitialReadings> for InitialReadingsResponse {
    fn from(r: bhtune_db::models::TuneRunInitialReadings) -> Self {
        InitialReadingsResponse {
            pv_ini: r.pv_ini,
            mv_ini: r.mv_ini,
            mv_range_low: r.mv_range_low,
            mv_range_high: r.mv_range_high,
            pv_range_high: r.pv_range_high,
            pv_range_low: r.pv_range_low,
            controller_direction: r.controller_direction,
            mode_raw: r.mode_raw,
            mode_attribute_raw: r.mode_attribute_raw,
            setpoint_ini: r.setpoint_ini,
        }
    }
}

/// One recorded tick: the [`Tick`] input and resulting engine state, plus the backend-
/// reported PV quality at read time. `Tick`/`MrftState` already derive `Serialize` in
/// `bhtune-core` (they round-trip through golden-trace fixtures too), so they're embedded
/// directly rather than re-projected field-by-field like the other DTOs here.
#[derive(Debug, Serialize)]
pub struct SampleResponse {
    pub tick_index: i64,
    pub sample: Tick,
    pub state: bhtune_core::MrftState,
    pub pv_quality: SampleQuality,
}

impl From<&TuneSampleRow> for SampleResponse {
    fn from(row: &TuneSampleRow) -> Self {
        SampleResponse {
            tick_index: row.tick_index,
            sample: row.sample,
            state: row.state,
            pv_quality: row.pv_quality,
        }
    }
}

/// Local projection of [`TuneResultRow`].
#[derive(Debug, Serialize)]
pub struct ResultResponse {
    pub response_level: ResponseLevel,
    pub kp: f32,
    pub ti_minutes: f32,
    pub td_minutes: f32,
    pub proportional: f32,
    pub integral: f32,
    pub derivative: f32,
}

impl From<&TuneResultRow> for ResultResponse {
    fn from(r: &TuneResultRow) -> Self {
        ResultResponse {
            response_level: r.response_level,
            kp: r.kp,
            ti_minutes: r.ti_minutes,
            td_minutes: r.td_minutes,
            proportional: r.proportional,
            integral: r.integral,
            derivative: r.derivative,
        }
    }
}

/// Local projection of [`TuneWriteRow`].
#[derive(Debug, Serialize)]
pub struct WriteResponse {
    pub kind: WriteKind,
    pub response_level: ResponseLevel,
    pub written_at: DateTime<Utc>,
    pub proportional_previous: Option<f32>,
    pub integral_previous: Option<f32>,
    pub derivative_previous: Option<f32>,
    pub proportional_written: Option<f32>,
    pub integral_written: Option<f32>,
    pub derivative_written: Option<f32>,
    pub proportional_readback: Option<f32>,
    pub integral_readback: Option<f32>,
    pub derivative_readback: Option<f32>,
    pub success: bool,
    pub error_message: Option<String>,
    pub rollback_state: Option<RollbackState>,
    pub rollback_error: Option<String>,
}

impl From<&TuneWriteRow> for WriteResponse {
    fn from(w: &TuneWriteRow) -> Self {
        WriteResponse {
            kind: w.kind,
            response_level: w.response_level,
            written_at: w.written_at,
            proportional_previous: w.previous.map(|p| p.proportional),
            integral_previous: w.previous.map(|p| p.integral),
            derivative_previous: w.previous.map(|p| p.derivative),
            proportional_written: w.proportional_written,
            integral_written: w.integral_written,
            derivative_written: w.derivative_written,
            proportional_readback: w.proportional_readback,
            integral_readback: w.integral_readback,
            derivative_readback: w.derivative_readback,
            success: w.success,
            error_message: w.error_message.clone(),
            rollback_state: w.rollback_state,
            rollback_error: w.rollback_error.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RunDetailResponse {
    pub id: i64,
    pub loop_name: String,
    pub backend: TuneBackend,
    pub outcome: TuneOutcome,
    pub failure_reason: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Name of the template snapshotted onto this run at start time -- not necessarily what
    /// `template_name` currently resolves to in the catalog (`safety-run-snapshot`).
    pub template_name: String,
    pub template_origin: TemplateOrigin,
    pub config: LoopConfig,
    pub initial_readings: Option<InitialReadingsResponse>,
    pub samples: Vec<SampleResponse>,
    pub results: Vec<ResultResponse>,
    pub writes: Vec<WriteResponse>,
    pub restore_status: Option<RestoreStatus>,
    pub restore_detail: Option<String>,
}

/// `GET /api/runs/{id}` -- 404 if no run has that id.
async fn show_run(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
) -> Result<Json<RunDetailResponse>, ApiError> {
    let run = TuneRunRow::get(&state.pool, run_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no run with id {run_id}")))?;
    let samples = TuneSampleRow::list_for_run(&state.pool, run_id).await?;
    let results = TuneResultRow::list_for_run(&state.pool, run_id).await?;
    let writes = TuneWriteRow::list_for_run(&state.pool, run_id).await?;

    Ok(Json(RunDetailResponse {
        id: run.id,
        loop_name: run.loop_name,
        backend: run.backend,
        outcome: run.outcome,
        failure_reason: run.failure_reason,
        started_at: run.started_at,
        completed_at: run.completed_at,
        template_name: run.template.name,
        template_origin: run.template_origin,
        config: run.config,
        initial_readings: run.initial_readings.map(InitialReadingsResponse::from),
        samples: samples.iter().map(SampleResponse::from).collect(),
        results: results.iter().map(ResultResponse::from).collect(),
        writes: writes.iter().map(WriteResponse::from).collect(),
        restore_status: run.restore_status,
        restore_detail: run.restore_detail,
    }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/runs", get(list_runs))
        .route("/api/runs/{id}", get(show_run))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn seed_one_run(state: &AppState) -> i64 {
        let template_row =
            bhtune_db::models::DcsTemplateRow::get_by_name(&state.pool, "Yokogawa CentumVP")
                .await
                .unwrap()
                .unwrap();
        let template = template_row.template;
        let config = LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 5.0,
            num_cycles_skip: 1,
            num_cycles_count: 3,
            noise_protection_secs: 0,
            mrft_delay_secs: 0,
        };
        let tags = bhtune_core::LoopTags::derive_from_pv_tag("Loop1.PV", &template);
        let run = TuneRunRow::start(
            &state.pool,
            None,
            "Loop1",
            TuneBackend::Simulator,
            config,
            template_row.origin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        run.id
    }

    /// Seeds a run carrying real data in every optional slot `seed_one_run` leaves empty --
    /// initial readings, a sample, a result, and both a successful and a rolled-back write --
    /// so every `From` impl in this module and every `filter_from_query` branch actually runs
    /// at least once. Returns `(run_id, loop_id)`: unlike `seed_one_run`'s ad hoc
    /// `loop_id = None` run, this one is attached to a real `loops` row so the `loop_id`
    /// filter has something to match against.
    async fn seed_full_run(state: &AppState) -> (i64, i64) {
        let template_row =
            bhtune_db::models::DcsTemplateRow::get_by_name(&state.pool, "Yokogawa CentumVP")
                .await
                .unwrap()
                .unwrap();
        let template = template_row.template;
        let config = LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 5.0,
            num_cycles_skip: 1,
            num_cycles_count: 3,
            noise_protection_secs: 0,
            mrft_delay_secs: 0,
        };
        let tags = bhtune_core::LoopTags::derive_from_pv_tag("Loop2.PV", &template);
        let now = Utc::now();

        let loop_id = sqlx::query(
            r#"
            INSERT INTO loops (
                name, dcs_template_id, tags_json, process_type, controller_type,
                relay_amp_percent, num_cycles_skip, num_cycles_count, noise_protection_secs,
                mrft_delay_secs, created_at, updated_at
            ) VALUES ('Loop2', ?, '{}', 'flow', 'pi', 5.0, 1, 3, 0, 0, ?, ?)
            "#,
        )
        .bind(template_row.id)
        .bind(now)
        .bind(now)
        .execute(&state.pool)
        .await
        .unwrap()
        .last_insert_rowid();

        let run = TuneRunRow::start(
            &state.pool,
            Some(loop_id),
            "Loop2",
            TuneBackend::Simulator,
            config,
            template_row.origin,
            &template,
            &tags,
            now,
        )
        .await
        .unwrap();
        let run_id = run.id;

        TuneRunRow::record_initial_readings(
            &state.pool,
            run_id,
            bhtune_db::models::TuneRunInitialReadings {
                pv_ini: 50.0,
                mv_ini: 45.0,
                mv_range_low: 0.0,
                mv_range_high: 100.0,
                pv_range_high: 100.0,
                pv_range_low: 0.0,
                controller_direction: ControllerDirection::Direct,
                mode_raw: Some("1".to_string()),
                mode_attribute_raw: Some("2".to_string()),
                setpoint_ini: Some(50.0),
            },
        )
        .await
        .unwrap();

        TuneSampleRow::insert(
            &state.pool,
            run_id,
            0,
            Tick {
                time: now,
                pv: 50.0,
            },
            bhtune_core::MrftState {
                hysteresis: 1.0,
                mv_value_current: 45.0,
                mv_sign_next_step: 1,
                counter_all_switches: 0,
                cycles_completed: 0,
                cycles_remaining: 3,
            },
            SampleQuality::Good,
        )
        .await
        .unwrap();

        TuneResultRow::insert(
            &state.pool,
            &TuneResultRow {
                id: 0,
                run_id,
                response_level: ResponseLevel::Moderate,
                kp: 1.5,
                ti_minutes: 2.0,
                td_minutes: 0.0,
                proportional: 66.7,
                integral: 2.0,
                derivative: 0.0,
            },
        )
        .await
        .unwrap();

        // A confirmed write: previous/written/readback all populated, no rollback needed.
        let mut successful_write =
            bhtune_db::models::NewTuneWrite::new(ResponseLevel::Moderate, now);
        successful_write.previous = Some(bhtune_db::models::WriteReadback {
            proportional: 60.0,
            integral: 3.0,
            derivative: 0.0,
        });
        successful_write.proportional_written = Some(66.7);
        successful_write.integral_written = Some(2.0);
        successful_write.derivative_written = Some(0.0);
        successful_write.proportional_readback = Some(66.7);
        successful_write.integral_readback = Some(2.0);
        successful_write.derivative_readback = Some(0.0);
        successful_write.success = true;
        TuneWriteRow::insert(&state.pool, run_id, successful_write)
            .await
            .unwrap();

        // A rejected write that was rolled back: readback stays unset (never confirmed) and
        // `rollback_state` is populated -- exercises the `Option`/`success = false` branches
        // `WriteResponse::from` otherwise never reaches.
        let mut failed_write = bhtune_db::models::NewTuneWrite::new(ResponseLevel::Aggressive, now);
        failed_write.previous = Some(bhtune_db::models::WriteReadback {
            proportional: 60.0,
            integral: 3.0,
            derivative: 0.0,
        });
        failed_write.proportional_written = Some(100.0);
        failed_write.success = false;
        failed_write.error_message = Some("write rejected: value out of range".to_string());
        failed_write.rollback_state = Some(RollbackState::Succeeded);
        TuneWriteRow::insert(&state.pool, run_id, failed_write)
            .await
            .unwrap();

        TuneRunRow::complete(&state.pool, run_id, now)
            .await
            .unwrap();

        (run_id, loop_id)
    }

    #[tokio::test]
    async fn list_runs_returns_empty_when_no_runs_exist() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let response = app
            .oneshot(Request::get("/api/runs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["total"], 0);
        assert_eq!(body["returned"], 0);
        assert!(body["runs"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_runs_returns_a_seeded_run() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_one_run(&state).await;
        let app = router().with_state(state);
        let response = app
            .oneshot(Request::get("/api/runs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["total"], 1);
        assert_eq!(body["runs"][0]["id"], run_id);
        assert_eq!(body["runs"][0]["process_type"], "flow");
    }

    #[tokio::test]
    async fn list_runs_filters_by_outcome() {
        let state = crate::test_support::in_memory_state().await;
        seed_one_run(&state).await;
        let app = router().with_state(state);
        let response = app
            .oneshot(
                Request::get("/api/runs?outcome=completed")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        // The seeded run is still `running` (never completed), so filtering for `completed`
        // must exclude it.
        assert_eq!(body["total"], 0);
    }

    #[tokio::test]
    async fn show_run_returns_full_detail() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_one_run(&state).await;
        let app = router().with_state(state);
        let response = app
            .oneshot(
                Request::get(format!("/api/runs/{run_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["id"], run_id);
        assert_eq!(body["template_name"], "Yokogawa CentumVP");
        assert!(body["samples"].as_array().unwrap().is_empty());
        assert!(body["results"].as_array().unwrap().is_empty());
        assert!(body["writes"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn show_run_returns_full_detail_with_samples_results_and_writes() {
        let state = crate::test_support::in_memory_state().await;
        let (run_id, _loop_id) = seed_full_run(&state).await;
        let app = router().with_state(state);
        let response = app
            .oneshot(
                Request::get(format!("/api/runs/{run_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;

        assert_eq!(body["id"], run_id);
        assert_eq!(body["outcome"], "completed");
        assert!(body["completed_at"].is_string());

        let initial_readings = &body["initial_readings"];
        assert_eq!(initial_readings["pv_ini"], 50.0);
        assert_eq!(initial_readings["mv_ini"], 45.0);
        assert_eq!(initial_readings["controller_direction"], "direct");
        assert_eq!(initial_readings["mode_raw"], "1");
        assert_eq!(initial_readings["mode_attribute_raw"], "2");
        assert_eq!(initial_readings["setpoint_ini"], 50.0);

        let samples = body["samples"].as_array().unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0]["tick_index"], 0);
        assert_eq!(samples[0]["sample"]["pv"], 50.0);
        assert_eq!(samples[0]["state"]["mv_value_current"], 45.0);
        assert_eq!(samples[0]["pv_quality"], "good");

        let results = body["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["response_level"], "moderate");
        assert_eq!(results[0]["kp"], 1.5);
        assert_eq!(results[0]["proportional"], 66.7);

        let writes = body["writes"].as_array().unwrap();
        assert_eq!(writes.len(), 2);
        let successful = writes
            .iter()
            .find(|w| w["response_level"] == "moderate")
            .unwrap();
        assert_eq!(successful["success"], true);
        assert_eq!(successful["proportional_previous"], 60.0);
        assert_eq!(successful["proportional_readback"], 66.7);
        assert!(successful["rollback_state"].is_null());
        assert!(successful["error_message"].is_null());

        let failed = writes
            .iter()
            .find(|w| w["response_level"] == "aggressive")
            .unwrap();
        assert_eq!(failed["success"], false);
        assert_eq!(failed["proportional_written"], 100.0);
        assert!(failed["proportional_readback"].is_null());
        assert_eq!(failed["rollback_state"], "succeeded");
        assert_eq!(
            failed["error_message"],
            "write rejected: value out of range"
        );
    }

    #[tokio::test]
    async fn list_runs_filters_by_every_supported_query_parameter_simultaneously() {
        let state = crate::test_support::in_memory_state().await;
        let (run_id, loop_id) = seed_full_run(&state).await;
        // A second, unrelated run proves the filters actually narrow the result set rather
        // than just happening to match everything in an otherwise-empty database.
        seed_one_run(&state).await;
        let app = router().with_state(state);

        let started_after = (Utc::now() - chrono::Duration::hours(1))
            .to_rfc3339()
            .replace('+', "%2B");
        let started_before = (Utc::now() + chrono::Duration::hours(1))
            .to_rfc3339()
            .replace('+', "%2B");
        let uri = format!(
            "/api/runs?loop_id={loop_id}&process_type=flow&controller_type=pi\
             &outcome=completed&backend=simulator&started_after={started_after}\
             &started_before={started_before}&template_name=Yokogawa+CentumVP\
             &template_origin=builtin"
        );

        let response = app
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["total"], 1);
        assert_eq!(body["runs"][0]["id"], run_id);
    }

    #[tokio::test]
    async fn show_run_404s_for_an_unknown_id() {
        let app = router().with_state(crate::test_support::in_memory_state().await);
        let response = app
            .oneshot(
                Request::get("/api/runs/999999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
