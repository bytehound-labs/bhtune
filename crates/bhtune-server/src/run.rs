//! The actual `bhtune-server` bootstrap-and-serve sequence, split out of `main.rs` so it can
//! be driven two different ways (`server-windows-service`): directly, from an interactive
//! console session or a systemd/launchd-managed foreground process, or from inside the
//! Windows Service Control Manager's own callback thread (`crate::service`'s
//! `#[cfg(windows)]` glue), which needs its *own* shutdown trigger (an SCM Stop/Shutdown
//! control event) instead of Ctrl+C/`SIGTERM`.
//!
//! Split into two phases rather than one long function, so a caller that needs to know the
//! exact moment the server is actually ready to accept connections (the Windows service path
//! reports `SERVICE_RUNNING` to the SCM at that point, not a moment earlier) can await
//! [`build_server`] and only then move on -- see [`BoundServer`].

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use bhtune_cli::{config, db, logging};

use crate::active_run::ActiveRun;
use crate::{AppState, build_router};

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

/// A `bhtune-server` that has finished every startup step through binding its listening
/// socket -- config/log/db resolution, migrations, template/retention seeding -- and is
/// ready to actually start accepting connections, but has not yet been handed to
/// [`serve`]. Returned as one value (rather than `serve` doing all of this itself) so a
/// caller can observe "fully bound and ready" as a distinct moment from "now serving until
/// told to stop" -- see this module's doc comment.
pub struct BoundServer {
    listener: tokio::net::TcpListener,
    app: axum::Router,
    active_run: ActiveRun,
    // Held for as long as `BoundServer` (and, transitively, whatever `serve` destructures it
    // into) lives -- dropping it any earlier risks silently truncating buffered log lines,
    // per `logging::init_tracing`'s own doc comment. Not unwrapped, matching the original
    // `main.rs`'s own `let _log_guard = logging::init_tracing(..)` -- a logging setup failure
    // (e.g. an unwritable log directory) shouldn't itself prevent the server from starting.
    log_guard: anyhow::Result<tracing_appender::non_blocking::WorkerGuard>,
}

/// Runs every step of `bhtune-server`'s startup through binding its listening socket:
/// resolve config (an explicit `config_path` if given, otherwise the platform's
/// auto-discovered path -- mirroring `bhtune-cli` calling `load_config(None)` whenever
/// `--config` itself wasn't passed), init logging, open/migrate/seed the database, spawn the
/// periodic retention sweeper, and bind the configured address.
///
/// Does not start serving -- see [`serve`].
pub async fn build_server(config_path: Option<&Path>) -> anyhow::Result<BoundServer> {
    let loaded_config = config::load_config_store(config_path)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let config = loaded_config.config.clone();

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
    let log_guard = logging::init_tracing(&log_settings);

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

    let config_store = Arc::new(RwLock::new(loaded_config));
    spawn_retention_sweeper(pool.clone(), config_store.clone());

    let bind_addr = config::resolve_bind_addr(std::env::var("BHTUNE_BIND").ok(), &config);
    let addr: SocketAddr = bind_addr
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid bind address '{bind_addr}': {e}"))?;

    let active_run = ActiveRun::default();
    let app = build_router(AppState {
        pool,
        active_run: active_run.clone(),
        config_store,
    });
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // Logs the OS-assigned address, not the requested `addr` -- identical for every real
    // deployment (a concrete port is always configured), but the two differ whenever the
    // requested port is `0` (bind to any free port), which is exactly what lets tests avoid
    // hardcoding a port that might collide with something else already listening.
    let local_addr = listener.local_addr()?;
    tracing::info!(%local_addr, "bhtune-server listening");
    println!("bhtune-server listening on http://{local_addr}");

    Ok(BoundServer {
        listener,
        app,
        active_run,
        log_guard,
    })
}

/// Serves `server` until `shutdown` resolves, then drains in-flight HTTP connections and
/// cancels/waits for any still-active tune run before returning -- the interactive path's
/// `main.rs` awaits [`shutdown_signal`]; the Windows service path
/// (`crate::service::windows_impl`) awaits its own SCM-driven signal instead.
pub async fn serve(
    server: BoundServer,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    // `_log_guard` is never read again, only held: it must simply outlive every `tracing`
    // call this function makes below, which binding it (rather than discarding it with `_`
    // in the destructuring pattern, which would drop it immediately) guarantees -- it goes
    // out of scope, and only then flushes/joins its writer thread, when this function
    // returns.
    let BoundServer {
        listener,
        app,
        active_run,
        log_guard: _log_guard,
    } = server;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;

    // Runs *after* axum has finished draining in-flight HTTP connections, not folded into
    // the shutdown future itself -- so a client mid-`GET /api/runs/:id` during shutdown still
    // gets its response before this starts cancelling the run it might have been asking
    // about.
    active_run
        .cancel_and_wait(SHUTDOWN_RUN_CANCEL_TIMEOUT)
        .await;

    Ok(())
}

/// Waits for Ctrl+C (SIGINT), or on Unix, SIGTERM -- so a service manager's ordinary "stop"
/// request (`systemctl stop`, `launchctl stop`) drains in-flight requests the same way an
/// interactive Ctrl+C does, rather than dropping connections mid-response. The Windows
/// Service Control Manager's own Stop/Shutdown control codes don't arrive as either of these
/// signals -- see `crate::service::windows_impl::run_service` for the SCM-specific
/// equivalent used instead when running as a Windows service.
pub async fn shutdown_signal() {
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
/// [`RETENTION_SWEEP_INTERVAL`] for as long as the server keeps running. The task retains the
/// synchronized store rather than a copied day count, so a config-page save is observed by
/// the next sweep; a disabled policy simply makes that tick a no-op.
///
/// Not joined or cancelled anywhere: the task only ever does one cheap `DELETE` per tick and
/// holds no resources between ticks, so letting it end abruptly when the process exits
/// (rather than folding it into a careful graceful-shutdown sequence) risks losing at most
/// one in-progress sweep, never corrupting anything -- SQLite's own transaction guarantees
/// cover the rest.
fn spawn_retention_sweeper(
    pool: bhtune_db::SqlitePool,
    config_store: Arc<RwLock<bhtune_cli::config::LoadedConfigStore>>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(RETENTION_SWEEP_INTERVAL);
        // The first tick fires immediately; `db::open` already ran a startup sweep moments
        // ago, so this first iteration would otherwise be a guaranteed-redundant no-op.
        interval.tick().await;
        loop {
            interval.tick().await;
            retention_tick_live(&pool, &config_store).await;
        }
    });
}

async fn retention_tick_live(
    pool: &bhtune_db::SqlitePool,
    config_store: &Arc<RwLock<bhtune_cli::config::LoadedConfigStore>>,
) {
    let config = match config_store.read() {
        Ok(store) => store.config.clone(),
        Err(_) => {
            tracing::warn!("configuration store lock poisoned; skipping retention sweep");
            return;
        }
    };
    let env_days = std::env::var("BHTUNE_RETENTION_DAYS")
        .ok()
        .and_then(|value| value.parse().ok());
    let Some(days) = bhtune_cli::config::resolve_retention_days(env_days, &config) else {
        return;
    };
    retention_tick(pool, days).await;
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

    #[test]
    fn shutdown_and_retention_intervals_match_the_documented_policy() {
        assert_eq!(SHUTDOWN_RUN_CANCEL_TIMEOUT, Duration::from_secs(35));
        assert_eq!(RETENTION_SWEEP_INTERVAL, Duration::from_secs(24 * 60 * 60));
    }

    #[tokio::test]
    async fn build_server_propagates_a_malformed_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        std::fs::write(&path, "retention_days = [not valid toml").unwrap();

        let result = build_server(Some(&path)).await;
        assert!(result.is_err());
        let error = result.err().unwrap();
        assert!(error.to_string().contains("failed to parse config file"));
    }

    #[tokio::test]
    async fn build_server_propagates_a_missing_explicit_template_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("bhtune.toml");
        let templates_path = dir.path().join("missing-templates.toml");
        std::fs::write(&config_path, format!("templates = {:?}\n", templates_path)).unwrap();

        let result = build_server(Some(&config_path)).await;
        assert!(result.is_err());
        let error = result.err().unwrap();
        assert!(error.to_string().contains("templates file not found"));
    }

    async fn insert_old_run(pool: &bhtune_db::SqlitePool) -> i64 {
        use bhtune_core::{ControllerType, LoopConfig, LoopTags, ProcessType, built_in_templates};
        use bhtune_db::models::{TemplateOrigin, TuneDriver, TuneRunRow};

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
        TuneRunRow::start(
            pool,
            None,
            "LIC-X",
            TuneDriver::Simulator,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            old_started_at,
        )
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn retention_tick_deletes_runs_past_the_cutoff_and_logs_nothing_fatal() {
        let pool = connect_in_memory().await.unwrap();
        let old_run_id = insert_old_run(&pool).await;

        retention_tick(&pool, 30).await;

        assert!(
            bhtune_db::models::TuneRunRow::get(&pool, old_run_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn retention_tick_on_a_pool_with_no_matching_runs_is_a_silent_no_op() {
        let pool = connect_in_memory().await.unwrap();
        // Nothing to delete, and no way for this to fail -- just confirms the helper
        // returns cleanly rather than panicking on an empty database.
        retention_tick(&pool, 30).await;
    }

    #[tokio::test]
    async fn build_server_completes_startup_and_can_serve_until_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("bhtune.toml");
        let db_path = dir.path().join("bhtune.db");
        let log_dir = dir.path().join("logs");
        std::fs::write(
            &config_path,
            format!(
                "db = {:?}\nbind = \"127.0.0.1:0\"\n[log]\ndir = {:?}\n",
                db_path, log_dir
            ),
        )
        .unwrap();

        let server = build_server(Some(&config_path)).await.unwrap();
        serve(server, async {}).await.unwrap();
    }

    #[tokio::test]
    async fn retention_tick_live_runs_when_retention_is_configured() {
        let pool = connect_in_memory().await.unwrap();
        let old_run_id = insert_old_run(&pool).await;
        let store = Arc::new(RwLock::new(bhtune_cli::config::LoadedConfigStore {
            path: None,
            missing_is_allowed: true,
            original_raw: None,
            config: bhtune_cli::config::BhtuneConfig {
                retention_days: Some(30),
                ..Default::default()
            },
            revision: "revision".to_string(),
            toml_allow_uncertain_quality: None,
        }));

        retention_tick_live(&pool, &store).await;

        assert!(
            bhtune_db::models::TuneRunRow::get(&pool, old_run_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn retention_tick_live_skips_when_retention_is_disabled() {
        let pool = connect_in_memory().await.unwrap();
        let store = Arc::new(RwLock::new(bhtune_cli::config::LoadedConfigStore {
            path: None,
            missing_is_allowed: true,
            original_raw: None,
            config: Default::default(),
            revision: "revision".to_string(),
            toml_allow_uncertain_quality: None,
        }));

        retention_tick_live(&pool, &store).await;
    }

    #[tokio::test]
    async fn retention_tick_live_skips_when_config_store_lock_is_poisoned() {
        let pool = connect_in_memory().await.unwrap();
        let store = Arc::new(RwLock::new(bhtune_cli::config::LoadedConfigStore {
            path: None,
            missing_is_allowed: true,
            original_raw: None,
            config: Default::default(),
            revision: "revision".to_string(),
            toml_allow_uncertain_quality: None,
        }));
        let poisoned = Arc::clone(&store);
        std::thread::spawn(move || {
            let _guard = poisoned.write().unwrap();
            panic!("poison configuration store");
        })
        .join()
        .unwrap_err();

        retention_tick_live(&pool, &store).await;
    }

    #[tokio::test]
    async fn retention_tick_live_logs_and_returns_when_sweep_fails() {
        let pool = connect_in_memory().await.unwrap();
        pool.close().await;
        let store = Arc::new(RwLock::new(bhtune_cli::config::LoadedConfigStore {
            path: None,
            missing_is_allowed: true,
            original_raw: None,
            config: bhtune_cli::config::BhtuneConfig {
                retention_days: Some(30),
                ..Default::default()
            },
            revision: "revision".to_string(),
            toml_allow_uncertain_quality: None,
        }));

        retention_tick_live(&pool, &store).await;
    }

    #[tokio::test(start_paused = true)]
    async fn spawned_retention_sweeper_runs_a_periodic_tick() {
        tokio::time::resume();
        let pool = connect_in_memory().await.unwrap();
        let old_run_id = insert_old_run(&pool).await;
        tokio::time::pause();
        let store = Arc::new(RwLock::new(bhtune_cli::config::LoadedConfigStore {
            path: None,
            missing_is_allowed: true,
            original_raw: None,
            config: bhtune_cli::config::BhtuneConfig {
                retention_days: Some(30),
                ..Default::default()
            },
            revision: "revision".to_string(),
            toml_allow_uncertain_quality: None,
        }));

        spawn_retention_sweeper(pool.clone(), store);
        tokio::task::yield_now().await;
        tokio::time::advance(RETENTION_SWEEP_INTERVAL + Duration::from_secs(1)).await;
        tokio::time::resume();
        for _ in 0..3 {
            tokio::task::yield_now().await;
        }

        assert!(
            bhtune_db::models::TuneRunRow::get(&pool, old_run_id)
                .await
                .unwrap()
                .is_none()
        );
    }
}
