use bhtune_core::{MrftState, Tick};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::{
    convert::{enum_to_text, text_to_enum},
    error::{DbError, DbResult},
};

#[cfg(doc)]
use super::dcs_templates::TemplateOrigin;

// tune_samples {{{1

/// How much a [`TuneSampleRow`]'s `sample.pv` reading should be trusted, as recorded at the
/// moment it was read (finding 5 of the live-plant safety review).
///
/// A `bhtune-db`-local mirror of [`bhtune_driver::Quality`], not a reuse of it directly:
/// `bhtune-db` deliberately doesn't depend on `bhtune-driver` (a leaf I/O-adapter crate with
/// a much heavier dependency tree -- `tokio`, `tonic`, `opcda-bridge` -- that has no business
/// in the persistence crate just to name one three-variant enum), so `bhtune-cli`, which
/// already depends on both, is the one place that converts between them. This mirrors
/// [`TemplateOrigin`]'s own precedent: a small, persistence-local enum rather than a second
/// dependency edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SampleQuality {
    Good,
    Uncertain,
    Bad,
}

/// One row of `tune_samples`: a single tick's [`Tick`] input and resulting [`MrftState`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TuneSampleRow {
    pub id: i64,
    pub run_id: i64,
    /// 0-based sample sequence number within the run. Named `tick_index` rather than `tick`
    /// to avoid colliding with the unrelated [`Tick`] type held in `sample`.
    pub tick_index: i64,
    pub sample: Tick,
    pub state: MrftState,
    /// The driver-reported quality of `sample.pv` at the moment it was read. See
    /// [`SampleQuality`].
    pub pv_quality: SampleQuality,
}

impl TuneSampleRow {
    /// Records one tick of a run, taking the exact [`Tick`]/[`MrftState`] pair
    /// [`bhtune_core::mrft::MrftEngine::step`] produced, plus the [`SampleQuality`] the
    /// driver reported for `sample.pv` at read time. `(run_id, tick_index)` is unique (see
    /// the migration), so re-recording the same tick twice is a caller bug, not a silent
    /// overwrite.
    pub async fn insert(
        pool: &SqlitePool,
        run_id: i64,
        tick_index: i64,
        sample: Tick,
        state: MrftState,
        pv_quality: SampleQuality,
    ) -> DbResult<TuneSampleRow> {
        let row = sqlx::query(
            r"
            INSERT INTO tune_samples (
                run_id, tick, time, pv, pv_quality, hysteresis, mv_value_current,
                mv_sign_next_step, counter_all_switches, cycles_completed, cycles_remaining
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING *
            ",
        )
        .bind(run_id)
        .bind(tick_index)
        .bind(sample.time)
        .bind(sample.pv)
        .bind(enum_to_text(&pv_quality)?)
        .bind(state.hysteresis)
        .bind(state.mv_value_current)
        .bind(state.mv_sign_next_step)
        .bind(state.counter_all_switches)
        .bind(state.cycles_completed)
        .bind(state.cycles_remaining)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

        row_to_tune_sample(row)
    }

    /// Lists every sample of `run_id`, ordered by tick — the full per-tick trend the history
    /// explorer's chart (`history-explorer-ui`) plots.
    pub async fn list_for_run(pool: &SqlitePool, run_id: i64) -> DbResult<Vec<TuneSampleRow>> {
        let rows = sqlx::query("SELECT * FROM tune_samples WHERE run_id = ? ORDER BY tick")
            .bind(run_id)
            .fetch_all(pool)
            .await
            .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_tune_sample).collect()
    }

    /// Lists only the samples of `run_id` recorded *after* `after_tick`, ordered by tick --
    /// what `bhtune-server`'s `GET /api/runs/{id}/stream` (`frontend-live-stream`) polls on
    /// every iteration so it never re-sends a tick it has already pushed to the browser.
    /// Pass `-1` to fetch every sample from the very first tick (`tune_samples.tick` is
    /// `>= 0`, so nothing is ever excluded by that sentinel).
    pub async fn list_for_run_since(
        pool: &SqlitePool,
        run_id: i64,
        after_tick: i64,
    ) -> DbResult<Vec<TuneSampleRow>> {
        let rows =
            sqlx::query("SELECT * FROM tune_samples WHERE run_id = ? AND tick > ? ORDER BY tick")
                .bind(run_id)
                .bind(after_tick)
                .fetch_all(pool)
                .await
                .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_tune_sample).collect()
    }
}

fn row_to_tune_sample(row: SqliteRow) -> DbResult<TuneSampleRow> {
    Ok(TuneSampleRow {
        id: row.try_get("id").map_err(DbError::Query)?,
        run_id: row.try_get("run_id").map_err(DbError::Query)?,
        tick_index: row.try_get("tick").map_err(DbError::Query)?,
        sample: Tick {
            time: row.try_get("time").map_err(DbError::Query)?,
            pv: row.try_get("pv").map_err(DbError::Query)?,
        },
        state: MrftState {
            hysteresis: row.try_get("hysteresis").map_err(DbError::Query)?,
            mv_value_current: row.try_get("mv_value_current").map_err(DbError::Query)?,
            mv_sign_next_step: row.try_get("mv_sign_next_step").map_err(DbError::Query)?,
            counter_all_switches: row
                .try_get("counter_all_switches")
                .map_err(DbError::Query)?,
            cycles_completed: row.try_get("cycles_completed").map_err(DbError::Query)?,
            cycles_remaining: row.try_get("cycles_remaining").map_err(DbError::Query)?,
        },
        pv_quality: {
            let pv_quality: String = row.try_get("pv_quality").map_err(DbError::Query)?;
            text_to_enum("pv_quality", &pv_quality)?
        },
    })
}
// }}}1
