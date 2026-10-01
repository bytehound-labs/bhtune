//! `POST /api/runs` (start a new tune run) and `POST /api/runs/{id}/cancel` (request its
//! cancellation) -- the write side of the run-history API `routes::history` reads from.
//!
//! Converts the HTTP request into the runtime's validated tune request, then uses the same
//! preparation and drive path as the CLI. Template lookup, tag derivation, common input
//! validation, driver connection, quality checks, restore-on-abort, and write-back rollback
//! therefore have one implementation; transport errors and reporting stay in this adapter.
//! `crate::active_run` tracks every in-flight run so each can be cancelled independently.

use crate::active_run::RunAlreadyActive;
use crate::error::{ApiError, ErrorBody, require_present};
use crate::routes::history::{RunDetailResponse, build_run_detail};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{post, put};
use axum::{Json, Router};
use bhtune_core::{ResponseLevel, opc_write_values};
use bhtune_db::SqlitePool;
use bhtune_db::models::{
    TuneDriver, TuneOutcome, TuneResultRow, TuneRunRow, WriteKind, WriteReadback,
};
use bhtune_driver::OpcDaDriver;
use bhtune_runtime::cancel::CtrlC;
use bhtune_runtime::tune::{
    DriverKind, PidWriteOutcome, drive, pid_parameters_for_result, prepare, write_pid_values,
};
use chrono::Utc;

mod dto;
mod handlers;
mod helpers;
mod validation;

pub use dto::{StartRunRequest, UpdateNotesRequest, WriteRunRequest};
pub(crate) use handlers::{
    __path_cancel_run, __path_delete_notes, __path_revert_run, __path_start_run,
    __path_update_notes, __path_write_run,
};
pub(crate) use handlers::{
    cancel_run, delete_notes, revert_run, start_run, update_notes, write_run,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/runs", post(start_run))
        .route("/api/runs/{id}/cancel", post(cancel_run))
        .route(
            "/api/runs/{id}/notes",
            put(update_notes).delete(delete_notes),
        )
        .route("/api/runs/{id}/write", post(write_run))
        .route("/api/runs/{id}/revert", post(revert_run))
}
