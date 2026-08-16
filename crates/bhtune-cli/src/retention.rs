//! `history-retention`: age-based deletion of old tune runs.
//!
//! The actual `DELETE` lives in [`bhtune_db::models::TuneRunRow::delete_matching`] (a single
//! statement, which SQLite already treats as its own transaction); this module owns the one
//! thing `bhtune-db` deliberately doesn't -- turning "N days" into a cutoff timestamp and
//! logging what happened, since `bhtune-db` has no logging dependency of its own (see its
//! crate doc comment).
//!
//! [`sweep_retention`] is the single code path shared by every caller that enforces the
//! policy, so "what a `bhtune history prune` run deletes", "what `crate::db::open`'s startup
//! sweep deletes", and "what `bhtune-server`'s periodic timer deletes" can never disagree:
//!
//! - `crate::db::open` calls it once, synchronously, on every startup of both binaries --
//!   the "on startup" half of the policy described in AGENTS.md's `history-retention` design
//!   note. A failure here is propagated (`?`), matching how that function already treats a
//!   failed template-seed as fatal: a one-shot CLI invocation failing fast and clearly beats
//!   silently skipping a maintenance step that might be masking a real database problem.
//! - `bhtune-server`'s `main.rs` additionally calls it on a periodic timer for as long as the
//!   process keeps running, so a long-lived server doesn't have to be restarted just to have
//!   its retention policy re-applied. Unlike the startup call, a failure there is logged and
//!   the timer keeps ticking -- crashing a process that's actively serving HTTP requests (and
//!   possibly mid-tune) over a background housekeeping error would be a far worse outcome
//!   than one skipped sweep.
//! - `bhtune history prune`'s non-`--dry-run` path calls it directly for an
//!   immediately-requested, possibly policy-overriding one-off sweep.

use bhtune_db::SqlitePool;
use bhtune_db::models::{TuneRunFilter, TuneRunRow};
use chrono::{DateTime, Duration, Utc};

/// The `started_at` cutoff for a `days`-day retention policy evaluated at `now`: runs
/// started at or before this instant are in scope for deletion. Pulled out of
/// [`sweep_retention`] so `commands::history::prune`'s `--dry-run` preview can compute and
/// display the exact same cutoff its non-dry-run sibling would actually delete against,
/// without needing a database handle to do it.
pub fn cutoff_for(days: u32, now: DateTime<Utc>) -> DateTime<Utc> {
    now - Duration::days(i64::from(days))
}

/// Deletes every tune run with `started_at` at or before the `days`-day cutoff (see
/// [`cutoff_for`]), along with -- via `ON DELETE CASCADE` -- its samples, results, and
/// write-back audit rows. Returns the number of runs deleted.
///
/// Logs at INFO when something was actually deleted (so deletions are never silent, per
/// `history-retention`'s design note) and at DEBUG otherwise, so a no-op sweep -- the common
/// case for an install well under its retention window -- doesn't add log noise at the
/// default level.
pub async fn sweep_retention(
    pool: &SqlitePool,
    days: u32,
    now: DateTime<Utc>,
) -> anyhow::Result<u64> {
    let cutoff = cutoff_for(days, now);
    let deleted =
        TuneRunRow::delete_matching(pool, &TuneRunFilter::default().with_started_before(cutoff))
            .await?;
    if deleted > 0 {
        tracing::info!(
            deleted,
            retention_days = days,
            %cutoff,
            "deleted tune runs past the configured retention policy"
        );
    } else {
        tracing::debug!(retention_days = days, %cutoff, "retention sweep found no runs to delete");
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bhtune_core::{ControllerType, LoopConfig, ProcessType, built_in_templates};
    use bhtune_db::connect_in_memory;
    use bhtune_db::models::{TemplateOrigin, TuneBackend};

    fn sample_config() -> LoopConfig {
        LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 5.0,
            num_cycles_skip: 1,
            num_cycles_count: 2,
            noise_protection_secs: 3,
            mrft_delay_secs: 0,
        }
    }

    async fn start_run_at(pool: &SqlitePool, started_at: DateTime<Utc>) -> i64 {
        let template = built_in_templates().remove(0);
        let tags = bhtune_core::LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &template);
        TuneRunRow::start(
            pool,
            None,
            "LIC-X",
            TuneBackend::Simulator,
            sample_config(),
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap()
        .id
    }

    #[test]
    fn cutoff_for_subtracts_the_given_number_of_days() {
        let now = Utc::now();
        assert_eq!(cutoff_for(30, now), now - Duration::days(30));
        assert_eq!(cutoff_for(0, now), now);
    }

    #[tokio::test]
    async fn sweep_retention_deletes_only_runs_at_or_before_the_cutoff() {
        let pool = connect_in_memory().await.unwrap();
        let now = Utc::now();
        let old = start_run_at(&pool, now - Duration::days(45)).await;
        let recent = start_run_at(&pool, now - Duration::days(1)).await;

        let deleted = sweep_retention(&pool, 30, now).await.unwrap();
        assert_eq!(deleted, 1);
        assert!(TuneRunRow::get(&pool, old).await.unwrap().is_none());
        assert!(TuneRunRow::get(&pool, recent).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn sweep_retention_with_nothing_past_the_cutoff_deletes_nothing() {
        let pool = connect_in_memory().await.unwrap();
        let now = Utc::now();
        let recent = start_run_at(&pool, now).await;

        let deleted = sweep_retention(&pool, 30, now).await.unwrap();
        assert_eq!(deleted, 0);
        assert!(TuneRunRow::get(&pool, recent).await.unwrap().is_some());
    }
}
