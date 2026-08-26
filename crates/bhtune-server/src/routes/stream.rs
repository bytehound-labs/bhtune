//! `GET /api/runs/{id}/stream` -- pushes per-tick engine state to the browser over
//! Server-Sent Events as a run progresses, so the frontend's live trend chart doesn't have
//! to poll `GET /api/runs/{id}` and re-fetch the whole (ever-growing) `samples` array once a
//! second, the way `frontend/src/api/runs.ts`'s `useRun` interim-substitute polling did
//! before this endpoint existed (`frontend-live-stream`, the last piece `frontend-screens`
//! was blocked on).
//!
//! SSE, not WebSocket: the flow is strictly server -> client, and SSE gives every browser
//! automatic reconnection on a dropped connection, survives ordinary HTTP proxies, and is
//! trivially inspectable with `curl -N` -- see AGENTS.md's "Web app architecture" section for
//! the full rationale.
//!
//! Implemented as an internal poll of `tune_samples`/`tune_runs`, **not** a broadcast channel
//! threaded through `bhtune-cli::commands::tune`'s already-heavily-tested tick loop --
//! deliberately, so this endpoint adds zero risk to the shared CLI/server tune-execution code
//! path (`run_polling_loop` keeps its existing, already-proven signature and test suite
//! untouched). At this project's documented data volumes (a pathological 2-hour run is
//! ~9,000 samples -- see AGENTS.md's "History explorer" notes) polling the database every
//! [`POLL_INTERVAL`] is negligible cost, not a premature optimization to avoid.

use std::convert::Infallible;
use std::time::Duration;

use async_stream::stream;
use axum::Router;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use bhtune_db::models::{TuneOutcome, TuneRunRow, TuneSampleRow};
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::ApiError;
use crate::routes::history::{InitialReadingsResponse, SampleResponse};
use crate::state::AppState;

/// How often the stream re-polls the database for newly recorded samples or a changed run
/// outcome. Well under the CLI's own default 800ms tick interval, so a connected client never
/// perceives added latency beyond this poll interval itself; see the module doc for why
/// polling the database is an acceptable (not merely expedient) choice at this project's data
/// volumes.
const POLL_INTERVAL: Duration = Duration::from_millis(300);

/// The final event emitted on every `GET /api/runs/{id}/stream` connection, named `done`,
/// immediately before the stream closes. Deliberately carries only the outcome rather than
/// duplicating the full run detail: the frontend already has `GET /api/runs/{id}` (via
/// `useRun`) for the config/results/write-back-audit shape, so `done` just signals "stop
/// listening and go refetch that" rather than growing this endpoint a second response shape.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RunStreamDone {
    outcome: TuneOutcome,
}

/// The stream's item type is `Result<Event, Infallible>` -- see the module doc for why a
/// real transport-level error is never produced (a DB failure mid-stream is represented as a
/// named `error` SSE event instead, followed by ending the stream). A tiny helper rather
/// than a turbofish (`Ok::<Event, Infallible>(event)`) at every `yield` site, since the
/// `stream!` macro can't infer `Infallible` from context on its own (every `yield` only ever
/// produces `Ok`, so there's no `Err(...)` arm anywhere for type inference to anchor on).
fn ok_event(event: Event) -> Result<Event, Infallible> {
    Ok(event)
}

/// Stream per-tick engine state for one run over Server-Sent Events.
///
/// `GET /api/runs/{id}/stream` -- 404 if no run has that id. Emits one `initial` event (JSON
/// body: [`InitialReadingsResponse`]) as soon as the driver's initial snapshot is persisted,
/// then a `sample` event (JSON body: [`SampleResponse`], the same shape
/// `GET /api/runs/{id}`'s `samples` array already uses) for every tick recorded so far and
/// every new tick recorded while connected, followed by exactly one final `done` event (JSON
/// body: [`RunStreamDone`]) once the run reaches a terminal outcome -- after which the
/// connection closes. Safe to open at any point in a run's lifecycle, including after it has
/// already finished: the initial snapshot (when available) and every sample are replayed once,
/// immediately followed by `done`.
#[utoipa::path(
    get,
    path = "/api/runs/{id}/stream",
    tag = "runs",
    params(
        ("id" = i64, Path, description = "Run id"),
    ),
    responses(
        (
            status = 200,
            description = "A `text/event-stream` with an optional `initial` event \
                (data: InitialReadingsResponse), `sample` events (data: SampleResponse), \
                and one final `done` event (data: RunStreamDone).",
            content_type = "text/event-stream",
            body = SampleResponse,
        ),
        (status = 404, description = "No run with that id.", body = crate::error::ErrorBody),
    ),
)]
pub(crate) async fn stream_run(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    if TuneRunRow::get(&state.pool, run_id).await?.is_none() {
        return Err(ApiError::NotFound(format!("no run with id {run_id}")));
    }

    let pool = state.pool.clone();
    let events = stream! {
        // `-1`: nothing sent yet, so the first poll below fetches every tick recorded so
        // far -- see `TuneSampleRow::list_for_run_since`'s own doc comment for why `-1` is a
        // safe "everything" sentinel (`tick` is always `>= 0`).
        let mut last_tick: i64 = -1;
        let mut sent_initial = false;
        loop {
            let run_outcome = match TuneRunRow::get(&pool, run_id).await {
                Ok(Some(run)) => {
                    if !sent_initial && let Some(initial) = run.initial_readings.as_ref() {
                        let response = InitialReadingsResponse::from(initial.clone());
                        match Event::default().event("initial").json_data(response) {
                            Ok(event) => {
                                sent_initial = true;
                                yield ok_event(event);
                            }
                            Err(err) => tracing::error!(
                                run_id,
                                error = %err,
                                "failed to encode initial readings as an SSE event"
                            ),
                        }
                    }
                    Some(run.outcome)
                }
                // Still running (or -- defensively -- the run vanished from under us mid-
                // stream, which cannot happen in practice since nothing ever deletes a
                // `tune_runs` row): continue to the sample query.
                Ok(_) => None,
                Err(err) => {
                    tracing::error!(
                        run_id,
                        error = %err,
                        "failed to poll tune_runs for the run stream; ending the stream"
                    );
                    yield ok_event(Event::default().event("error").data(err.to_string()));
                    break;
                }
            };

            match TuneSampleRow::list_for_run_since(&pool, run_id, last_tick).await {
                Ok(samples) => {
                    for sample in &samples {
                        last_tick = sample.tick_index;
                        let response = SampleResponse::from(sample);
                        match Event::default().event("sample").json_data(response) {
                            Ok(event) => yield ok_event(event),
                            Err(err) => tracing::error!(
                                run_id,
                                error = %err,
                                "failed to encode a tune sample as an SSE event"
                            ),
                        }
                    }
                }
                Err(err) => {
                    tracing::error!(
                        run_id,
                        error = %err,
                        "failed to poll tune_samples for the run stream; ending the stream"
                    );
                    yield ok_event(Event::default().event("error").data(err.to_string()));
                    break;
                }
            }

            if let Some(outcome) = run_outcome
                && outcome != TuneOutcome::Running
            {
                let done = RunStreamDone { outcome };
                if let Ok(event) = Event::default().event("done").json_data(done) {
                    yield ok_event(event);
                }
                break;
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    };

    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/runs/{id}/stream", get(stream_run))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use bhtune_core::{ControllerType, LoopConfig, LoopTags, MrftState, ProcessType, Tick};
    use bhtune_db::models::{SampleQuality, TuneDriver, TuneRunInitialReadings};
    use chrono::Utc;
    use tower::ServiceExt;

    async fn seed_running_run(state: &AppState, name: &str) -> i64 {
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
        let tags = LoopTags::derive_from_pv_tag(&format!("{name}.PV"), &template);
        let run = TuneRunRow::start(
            &state.pool,
            None,
            name,
            TuneDriver::Simulator,
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

    async fn insert_sample(state: &AppState, run_id: i64, tick: i64) {
        let sample = Tick {
            time: Utc::now(),
            pv: 50.0 + tick as f32,
        };
        let mrft_state = MrftState {
            hysteresis: 1.0,
            mv_value_current: 55.0,
            mv_sign_next_step: 1,
            counter_all_switches: tick as u32,
            cycles_completed: 0,
            cycles_remaining: 2,
        };
        TuneSampleRow::insert(
            &state.pool,
            run_id,
            tick,
            sample,
            mrft_state,
            SampleQuality::Good,
        )
        .await
        .unwrap();
    }

    async fn record_initial_readings(state: &AppState, run_id: i64) {
        TuneRunRow::record_initial_readings(
            &state.pool,
            run_id,
            TuneRunInitialReadings {
                pv_ini: 48.0,
                mv_ini: 42.0,
                mv_range_low: 0.0,
                mv_range_high: 100.0,
                pv_range_high: 100.0,
                pv_range_low: 0.0,
                controller_direction: bhtune_core::ControllerDirection::Reverse,
                mode_raw: Some("AUTO".to_string()),
                mode_attribute_raw: None,
                setpoint_ini: Some(50.0),
            },
        )
        .await
        .unwrap();
    }

    /// Parses a `text/event-stream` body into `(event, data)` pairs, in order. A minimal,
    /// deliberately non-general parser matching exactly the shape this module's own handler
    /// produces (one `event:`/`data:` line pair per event, `\n\n`-terminated) -- good enough
    /// for asserting on our own output without pulling in a full SSE-client crate just for
    /// tests.
    fn parse_sse(body: &str) -> Vec<(String, String)> {
        body.split("\n\n")
            .filter(|chunk| !chunk.trim().is_empty())
            .map(|chunk| {
                let mut event = String::new();
                let mut data = String::new();
                for line in chunk.lines() {
                    if let Some(rest) = line.strip_prefix("event:") {
                        event = rest.trim().to_string();
                    } else if let Some(rest) = line.strip_prefix("data:") {
                        data = rest.trim().to_string();
                    }
                }
                (event, data)
            })
            .collect()
    }

    #[tokio::test]
    async fn streaming_an_unknown_run_returns_404() {
        let state = crate::test_support::in_memory_state().await;
        let app = crate::build_router(state);
        let response = app
            .oneshot(
                Request::get("/api/runs/999/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn streaming_a_completed_run_replays_every_sample_then_emits_done() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_running_run(&state, "LIC-STREAM-1").await;
        insert_sample(&state, run_id, 0).await;
        insert_sample(&state, run_id, 1).await;
        TuneRunRow::complete(&state.pool, run_id, Utc::now())
            .await
            .unwrap();

        let app = crate::build_router(state);
        let response = app
            .oneshot(
                Request::get(format!("/api/runs/{run_id}/stream"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap(),
            "text/event-stream"
        );

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        let events = parse_sse(&body);

        let sample_events: Vec<_> = events.iter().filter(|(e, _)| e == "sample").collect();
        assert_eq!(
            sample_events.len(),
            2,
            "both pre-recorded samples must be replayed"
        );
        let first: serde_json::Value = serde_json::from_str(&sample_events[0].1).unwrap();
        assert_eq!(first["tick_index"], 0);
        let second: serde_json::Value = serde_json::from_str(&sample_events[1].1).unwrap();
        assert_eq!(second["tick_index"], 1);

        let (last_event, last_data) = events.last().expect("stream must emit at least `done`");
        assert_eq!(last_event, "done");
        let done: serde_json::Value = serde_json::from_str(last_data).unwrap();
        assert_eq!(done["outcome"], "completed");
    }

    #[tokio::test]
    async fn streaming_emits_initial_readings_before_replayed_samples() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_running_run(&state, "LIC-STREAM-INITIAL").await;
        record_initial_readings(&state, run_id).await;
        insert_sample(&state, run_id, 0).await;
        TuneRunRow::complete(&state.pool, run_id, Utc::now())
            .await
            .unwrap();

        let app = crate::build_router(state);
        let response = app
            .oneshot(
                Request::get(format!("/api/runs/{run_id}/stream"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let events = parse_sse(core::str::from_utf8(&bytes).unwrap());

        assert_eq!(events[0].0, "initial");
        let initial: serde_json::Value = serde_json::from_str(&events[0].1).unwrap();
        assert_eq!(initial["pv_ini"], 48.0);
        assert_eq!(initial["mv_ini"], 42.0);
        assert_eq!(events[1].0, "sample");
        assert_eq!(events[2].0, "done");
    }

    #[tokio::test]
    async fn streaming_a_run_with_no_samples_still_terminates_with_done() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_running_run(&state, "LIC-STREAM-2").await;
        TuneRunRow::abort(&state.pool, run_id, Utc::now())
            .await
            .unwrap();

        let app = crate::build_router(state);
        let response = app
            .oneshot(
                Request::get(format!("/api/runs/{run_id}/stream"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let events = parse_sse(core::str::from_utf8(&bytes).unwrap());
        assert_eq!(
            events.len(),
            1,
            "no samples were recorded, so only `done` is emitted"
        );
        assert_eq!(events[0].0, "done");
        let done: serde_json::Value = serde_json::from_str(&events[0].1).unwrap();
        assert_eq!(done["outcome"], "aborted");
    }

    #[tokio::test]
    async fn streaming_a_still_running_run_waits_for_it_to_finish_before_closing() {
        let state = crate::test_support::in_memory_state().await;
        let run_id = seed_running_run(&state, "LIC-STREAM-3").await;
        insert_sample(&state, run_id, 0).await;
        // Left `running` -- the stream must poll past at least one `POLL_INTERVAL` sleep
        // before observing a terminal outcome. A background task flips it to `completed`
        // shortly after the stream has almost certainly already seen it as `running` at
        // least once (the handler's own pre-flight `TuneRunRow::get` existence check runs
        // synchronously before this task is even spawned, so there is no race on "does the
        // run exist").
        let pool = state.pool.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            TuneRunRow::complete(&pool, run_id, Utc::now())
                .await
                .unwrap();
        });

        let app = crate::build_router(state);
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            app.oneshot(
                Request::get(format!("/api/runs/{run_id}/stream"))
                    .body(Body::empty())
                    .unwrap(),
            ),
        )
        .await
        .expect("the stream must eventually close on its own")
        .unwrap();

        let bytes = tokio::time::timeout(
            Duration::from_secs(5),
            to_bytes(response.into_body(), usize::MAX),
        )
        .await
        .expect("reading the full (finite) SSE body must not hang")
        .unwrap();
        let events = parse_sse(core::str::from_utf8(&bytes).unwrap());

        assert!(
            events
                .iter()
                .any(|(e, d)| e == "sample" && d.contains("\"tick_index\":0")),
            "the pre-recorded sample must have been replayed: {events:?}"
        );
        let (last_event, last_data) = events.last().unwrap();
        assert_eq!(last_event, "done");
        let done: serde_json::Value = serde_json::from_str(last_data).unwrap();
        assert_eq!(done["outcome"], "completed");
    }
}
