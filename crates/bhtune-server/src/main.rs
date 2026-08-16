//! `bhtune-server` binary: a thin bootstrap shell, mirroring `bhtune-cli`'s own `main.rs`/
//! `lib.rs::run` split -- see `bhtune_server`'s crate doc comment for why the actual routes
//! live in the lib target instead.
//!
//! Deliberately has no CLI argument parsing of its own yet (no `clap` dependency -- adding
//! one is a natural next step once real deployments show which flags are worth it; see
//! AGENTS.md's `server-http-api` scope note). Every setting is resolved exactly the way
//! `bhtune-cli` resolves it -- config file > env var > default -- by calling straight into
//! `bhtune_cli::config`/`db`/`logging`, so the two adapters can never silently disagree
//! about, say, where the database lives.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use bhtune_cli::{config, db, logging};
use bhtune_server::active_run::ActiveRun;
use bhtune_server::{AppState, build_router};

/// How long graceful shutdown waits for an in-flight tune run to actually finish cancelling
/// (its restore attempt included) after `axum::serve` itself has finished draining
/// in-flight HTTP connections, before giving up and exiting anyway -- see
/// [`ActiveRun::cancel_and_wait`]'s own doc comment for what "giving up" logs.
const SHUTDOWN_RUN_CANCEL_TIMEOUT: Duration = Duration::from_secs(35);

/// How often the server re-applies `history-retention`'s policy for as long as it keeps
/// running, on top of the one-shot sweep `db::open` already ran at startup. A day is far
/// more than frequent enough for an age-based-in-days policy -- the oldest a run can ever
/// linger past its cutoff is one interval -- while being infrequent enough that the sweep
/// never meaningfully competes with real traffic for the database.
const RETENTION_SWEEP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `bhtune-server` has no `--config` flag (or any flags) of its own yet, so this always
    // resolves through the platform-auto-discovered path -- matching `bhtune-cli` calling
    // `load_config(None)` whenever `--config` itself wasn't passed.
    let config = config::load_config(None).unwrap_or_default();

    let default_log_dir = config::default_log_dir_from(
        std::env::var("XDG_DATA_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        std::env::var("APPDATA").ok().as_deref(),
        cfg!(target_os = "windows"),
    );
    let log_settings = logging::resolve_log_settings(
        std::env::var("RUST_LOG").ok(),
        None,
        None,
        None,
        &config.log,
        &default_log_dir,
    );
    // Held for the whole process lifetime (this function's own scope, since `main` runs
    // until the server shuts down) -- dropping it any earlier risks silently truncating
    // buffered log lines, exactly as `bhtune_cli::logging::init_tracing`'s doc comment
    // warns.
    let _log_guard = logging::init_tracing(&log_settings);

    let db_path = config::resolve_db_path(
        std::env::var("BHTUNE_DB").ok().map(PathBuf::from),
        &config,
        std::env::var("XDG_DATA_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        std::env::var("APPDATA").ok().as_deref(),
        cfg!(target_os = "windows"),
    );
    let user_templates = config::load_user_templates(
        std::env::var("BHTUNE_TEMPLATES").ok().map(PathBuf::from),
        &config,
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        std::env::var("APPDATA").ok().as_deref(),
        cfg!(target_os = "windows"),
    )?;
    let retention_days = config::resolve_retention_days(
        std::env::var("BHTUNE_RETENTION_DAYS")
            .ok()
            .and_then(|s| s.parse().ok()),
        &config,
    );
    let pool = db::open(&db_path, user_templates, retention_days).await?;

    if let Some(days) = retention_days {
        spawn_retention_sweeper(pool.clone(), days);
    }

    let bind_addr = config::resolve_bind_addr(std::env::var("BHTUNE_BIND").ok(), &config);
    let addr: SocketAddr = bind_addr
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid bind address '{bind_addr}': {e}"))?;

    // Kept as a separate binding (not just read back out of `AppState` after `axum::serve`
    // returns) so the graceful-shutdown call below reads clearly as "the same registry the
    // whole app shared" rather than looking like it's reaching back into a consumed value.
    let active_run = ActiveRun::default();
    let app = build_router(AppState {
        pool,
        active_run: active_run.clone(),
        app_config: config,
    });
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // Logs the OS-assigned address, not the requested `addr` -- identical for every real
    // deployment (a concrete port is always configured), but the two differ whenever the
    // requested port is `0` (bind to any free port), which is exactly what lets tests avoid
    // hardcoding a port that might collide with something else already listening.
    let local_addr = listener.local_addr()?;
    tracing::info!(%local_addr, "bhtune-server listening");
    println!("bhtune-server listening on http://{local_addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // Runs *after* axum has finished draining in-flight HTTP connections, not folded into
    // `shutdown_signal` itself -- so a client mid-`GET /api/runs/:id` during shutdown still
    // gets its response before this starts cancelling the run it might have been asking
    // about.
    active_run
        .cancel_and_wait(SHUTDOWN_RUN_CANCEL_TIMEOUT)
        .await;

    Ok(())
}

/// Waits for Ctrl+C (SIGINT), or on Unix, SIGTERM -- so a service manager's ordinary "stop"
/// request (`systemctl stop`, and eventually the Windows Service Control Manager, see
/// `server-windows-service`) drains in-flight requests the same way an interactive Ctrl+C
/// does, rather than dropping connections mid-response.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutdown signal received, draining in-flight requests");
}

/// Spawns the background task that re-applies `history-retention`'s policy every
/// [`RETENTION_SWEEP_INTERVAL`] for as long as the server keeps running. Only ever called
/// when a retention policy is actually configured (`main`'s `if let Some(days)` guard) --
/// there is deliberately no task at all, not a task that immediately no-ops, when retention
/// is disabled (the default), matching `db::open`'s own "skip entirely" behavior for
/// `None`.
///
/// Not joined or cancelled anywhere: the task only ever does one cheap `DELETE` per tick and
/// holds no resources between ticks, so letting it end abruptly when the process exits
/// (rather than folding it into `main`'s otherwise-careful graceful-shutdown sequence) risks
/// losing at most one in-progress sweep, never corrupting anything -- SQLite's own
/// transaction guarantees cover the rest.
fn spawn_retention_sweeper(pool: bhtune_db::SqlitePool, days: u32) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(RETENTION_SWEEP_INTERVAL);
        // The first tick fires immediately; `db::open` already ran a startup sweep moments
        // ago, so this first iteration would otherwise be a guaranteed-redundant no-op.
        interval.tick().await;
        loop {
            interval.tick().await;
            retention_tick(&pool, days).await;
        }
    });
}

/// One periodic retention sweep. Logs a warning and returns on failure rather than
/// propagating -- unlike `db::open`'s startup sweep (fatal by design, since a one-shot CLI
/// invocation failing fast beats silently proceeding on what might be a broken database),
/// crashing a long-running server over a background maintenance hiccup would drop every
/// in-flight HTTP connection and any actively-running tune, a far worse outcome than
/// skipping one sweep and retrying at the next interval.
async fn retention_tick(pool: &bhtune_db::SqlitePool, days: u32) {
    let now = chrono::Utc::now();
    if let Err(e) = bhtune_cli::retention::sweep_retention(pool, days, now).await {
        tracing::warn!(error = %e, "periodic retention sweep failed; will retry next interval");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bhtune_db::connect_in_memory;

    #[tokio::test]
    async fn retention_tick_deletes_runs_past_the_cutoff_and_logs_nothing_fatal() {
        use bhtune_core::{ControllerType, LoopConfig, LoopTags, ProcessType, built_in_templates};
        use bhtune_db::models::{TemplateOrigin, TuneBackend, TuneRunRow};

        let pool = connect_in_memory().await.unwrap();
        let template = built_in_templates().remove(0);
        let tags = LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &template);
        let config = LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 5.0,
            num_cycles_skip: 1,
            num_cycles_count: 2,
            noise_protection_secs: 3,
            mrft_delay_secs: 0,
        };
        let old_started_at = chrono::Utc::now() - chrono::Duration::days(100);
        let old_run = TuneRunRow::start(
            &pool,
            None,
            "LIC-X",
            TuneBackend::Simulator,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            old_started_at,
        )
        .await
        .unwrap();

        retention_tick(&pool, 30).await;

        assert!(TuneRunRow::get(&pool, old_run.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn retention_tick_on_a_pool_with_no_matching_runs_is_a_silent_no_op() {
        let pool = connect_in_memory().await.unwrap();
        // Nothing to delete, and no way for this to fail -- just confirms the helper
        // returns cleanly rather than panicking on an empty database.
        retention_tick(&pool, 30).await;
    }
}
