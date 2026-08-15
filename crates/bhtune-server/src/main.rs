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
    let pool = db::open(&db_path, user_templates).await?;

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
