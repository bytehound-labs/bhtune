//! Spawns the real, compiled `bhtune` binary and sends it a genuine `SIGINT`, proving the
//! Ctrl+C-triggered abort path in `commands::tune::run_polling_loop` actually restores the
//! loop and reports `Aborted` end to end — including exiting with `EXIT_ABORTED` rather than
//! success, so an unattended caller (a scheduler, a CI job) can distinguish an interrupted
//! tune from a completed one without parsing stdout.
//!
//! This has to be a real subprocess: `tokio::signal::ctrl_c()` listens for the process's own
//! signal handler, and `cargo test` runs every test as one thread inside a single shared
//! process, so delivering a real `SIGINT` to "just one test" isn't possible in-process — it
//! would interrupt the entire test binary. Unix-only (there is no POSIX `SIGINT` on Windows;
//! CI runs `ubuntu-latest` only, see `AGENTS.md`), matching the project's existing
//! `#[cfg(unix)]`-style precedent for platform-specific test infrastructure.

#![cfg(unix)]

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;

#[tokio::test]
async fn ctrl_c_aborts_a_running_tune_and_restores_the_loop() {
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("bhtune.db");
    let config_path = db_dir.path().join("bhtune.toml");
    std::fs::write(&config_path, "[tuning]\npoll_interval_ms = 1000\n")
        .expect("failed to write test configuration");
    // Without this, `run()`'s logging setup (see `bhtune_cli::logging`) would resolve the
    // real platform default log directory (`~/.local/share/bhtune/logs` or similar) using
    // this test process's *actual* inherited environment, since `Command::new` inherits the
    // parent's env by default -- writing real files under the developer/CI machine's actual
    // home directory as a side effect of running this test.
    let log_dir = tempfile::tempdir().unwrap();

    let child = Command::new(env!("CARGO_BIN_EXE_bhtune"))
        .arg("--db")
        .arg(&db_path)
        .arg("--config")
        .arg(&config_path)
        .arg("--log-dir")
        .arg(log_dir.path())
        .args([
            "tune",
            "--tagname",
            "ignored-for-simulator",
            "--template",
            "Yokogawa CentumVP",
            "--process-type",
            "flow",
            "--controller-type",
            "pi",
            "--relay-amp",
            "10",
            "--cycles-skip",
            "1",
            "--cycles-count",
            "2",
            "--noise-protection-secs",
            "0",
            "--driver",
            "simulator",
            "--sim-gain",
            "1.0",
            "--sim-tau",
            "2.0",
            "--sim-dead-time",
            "5.0",
            "--pv-range-high",
            "100",
            "--pv-range-low",
            "0",
            "--mv-range-high",
            "100",
            "--mv-range-low",
            "0",
            "--direction",
            "reverse",
            // Slow enough (1s/tick) that the ~300ms head start below is comfortably inside
            // the very first poll wait, and the process can't possibly reach a relay switch
            // (let alone all 3 required to complete) before the signal arrives.
            "--notes",
            "ctrlc-abort-test",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn the bhtune binary");

    // Give the child time to parse args, open/seed the database, read initial values, and
    // reach `run_polling_loop`'s `tokio::select!` before signalling it.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // SAFETY: `child.id()` is a live PID for a process this test just spawned and still
    // owns; `SIGINT` is the same signal a terminal's Ctrl+C sends, which is exactly the
    // condition `tokio::signal::ctrl_c()` listens for.
    let kill_result = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) };
    assert_eq!(kill_result, 0, "failed to send SIGINT to the child process");

    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || child.wait_with_output()),
    )
    .await
    .expect("bhtune did not exit within 10s of receiving SIGINT")
    .expect("joining the wait_with_output task panicked")
    .expect("failed to wait for the bhtune child process");

    assert_eq!(
        output.status.code(),
        Some(bhtune_cli::EXIT_ABORTED as i32),
        "expected exit code {} (EXIT_ABORTED -- distinct from a clean completion, so an \
         operator/scheduler can tell a Ctrl+C abort apart from success), got {:?}. stderr: {}",
        bhtune_cli::EXIT_ABORTED,
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let mut stdout = String::new();
    (&output.stdout[..])
        .read_to_string(&mut stdout)
        .expect("stdout is not valid utf-8");
    assert!(
        stdout.contains("Tune aborted"),
        "expected the abort message on stdout, got: {stdout:?}"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("Tune aborted"),
        "the tune's own result output must stay on stdout only -- tracing's stderr mirroring \
         must never carry it, or a scheduler parsing stdout as `--output json` could be \
         corrupted by an interleaved copy on the other stream"
    );

    // The real subprocess is the one place `logging::init_tracing` actually installs a
    // subscriber in a fresh, conflict-free process (see `logging`'s own test module doc
    // comment on why unit tests can't assert this) -- confirms real log output actually
    // reached the rotating file under `--log-dir`, not just that the flag was accepted.
    let log_files: Vec<_> = std::fs::read_dir(log_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        !log_files.is_empty(),
        "expected at least one log file under --log-dir {:?}",
        log_dir.path()
    );

    let pool = bhtune_db::connect(&db_path).await.unwrap();
    let runs = bhtune_db::models::TuneRunRow::list(
        &pool,
        &bhtune_db::models::TuneRunFilter::default(),
        bhtune_db::models::Pagination::first(10),
    )
    .await
    .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].loop_name, "ignored-for-simulator");
    assert_eq!(runs[0].notes.as_deref(), Some("ctrlc-abort-test"));
    assert_eq!(runs[0].outcome, bhtune_db::models::TuneOutcome::Aborted);
    pool.close().await;
}
