//! Spawns the real, compiled `bhtune` binary and proves `tune --output json` emits *exactly
//! one* parseable JSON value on stdout, with nothing printed ahead of it
//! (`safety-json-contract`, finding 8 of the live-plant safety review).
//!
//! Before this finding was fixed, `maybe_write_back` (`commands::tune`) unconditionally
//! `println!`ed its status/prompt lines regardless of `--output`, so a JSON-mode run of
//! exactly the shape this test drives -- a simulator driver, which never has PID constant
//! tags configured (see `build_tags`'s `DriverKindArg::Simulator` arm) -- printed "No PID
//! constant tags configured for this run's driver/template; skipping write-back." on stdout
//! *before* the run's final JSON object, breaking `serde_json::from_str` for every
//! scripted/scheduled caller. This has to be a real subprocess rather than an in-process
//! `Command`-level check: capturing whether *any* prose reaches real stdout ahead of the
//! JSON object requires the process's actual stdout stream, not a return value.
//!
//! Uses the same fast-completing simulator parameters as `commands::tune`'s own
//! `fast_simulator_args()` test helper (short poll interval, short lag/dead-time) so the
//! whole subprocess run finishes in well under a second of real wall-clock time.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Spawns `bhtune tune` with the given extra arguments (appended after the fast-completing
/// simulator baseline) and returns `(exit_code, stdout, stderr)`. Shared by every test in
/// this file so each one only states what it adds/overrides.
fn run_fast_simulator_tune(extra_args: &[&str]) -> (Option<i32>, String, String) {
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("bhtune.db");
    // See `ctrlc_abort.rs`'s identical comment: without this, logging setup would resolve
    // the real platform default log directory using this test process's inherited
    // environment, writing real files under the developer/CI machine's actual home
    // directory as a side effect of running this test.
    let log_dir = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_bhtune"))
        .arg("--db")
        .arg(&db_path)
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
            "0.01",
            "--sim-dead-time",
            "0.025",
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
            "--poll-interval-ms",
            "5",
            "--notes",
            "json-contract-test",
        ])
        .args(extra_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("failed to spawn/run the bhtune binary");

    let mut stdout = String::new();
    (&output.stdout[..])
        .read_to_string(&mut stdout)
        .expect("stdout is not valid utf-8");
    let mut stderr = String::new();
    (&output.stderr[..])
        .read_to_string(&mut stderr)
        .expect("stderr is not valid utf-8");

    (output.status.code(), stdout, stderr)
}

#[tokio::test]
async fn tune_output_json_emits_exactly_one_parseable_json_value_on_stdout() {
    let (exit_code, stdout, stderr) = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::task::spawn_blocking(|| run_fast_simulator_tune(&["--output", "json"])),
    )
    .await
    .expect("bhtune did not exit within 30s")
    .expect("joining the spawn_blocking task panicked");

    assert_eq!(
        exit_code,
        Some(bhtune_cli::EXIT_SUCCESS as i32),
        "expected a clean completion (write-back is always skipped for the simulator \
         driver, which is still `Completed`, not a failure); stderr: {stderr}"
    );

    // The load-bearing assertion: `serde_json::from_str` on the *entire, trimmed* stdout
    // succeeds only if stdout is exactly one JSON value with nothing else around it --
    // catching both "prose printed before the object" (this finding's original bug) and
    // "prose printed after it".
    let trimmed = stdout.trim();
    let json: serde_json::Value = serde_json::from_str(trimmed).unwrap_or_else(|e| {
        panic!("stdout was not exactly one parseable JSON value: {e}\nstdout was: {stdout:?}")
    });

    assert_eq!(json["outcome"], "completed");
    assert_eq!(json["write_back"], "skipped");
    let detail = json["write_back_detail"]
        .as_str()
        .expect("write_back_detail must be a string, not null, when write-back was skipped");
    assert!(
        detail.contains("no PID constant tags configured"),
        "expected the suppressed 'no PID constant tags' reason to be folded into \
         write_back_detail instead of only being printed as prose, got: {detail:?}"
    );

    // Interactive prompts/status lines must never reach stdout in JSON mode either -- this
    // run never reaches the interactive branch at all (no PID tags means it's skipped
    // before `write_pid` is even inspected), but assert the negative anyway as a direct
    // regression guard on the bug this finding fixes.
    assert!(
        !stdout.contains("PID constant tags configured for this run's driver/template;"),
        "the 'no PID constant tags configured' prose must not be printed in JSON mode, only \
         folded into write_back_detail; got stdout: {stdout:?}"
    );
}

#[tokio::test]
async fn tune_output_table_is_plain_text_not_json() {
    // Sanity check that `--output` actually branches: the default `Table` format for the
    // exact same run must NOT be parseable as a single JSON value, proving the two tests in
    // this file are actually exercising different code paths rather than the format flag
    // being ignored.
    let (exit_code, stdout, stderr) = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::task::spawn_blocking(|| run_fast_simulator_tune(&[])),
    )
    .await
    .expect("bhtune did not exit within 30s")
    .expect("joining the spawn_blocking task panicked");

    assert_eq!(
        exit_code,
        Some(bhtune_cli::EXIT_SUCCESS as i32),
        "stderr: {stderr}"
    );
    assert!(
        stdout.contains("Tune completed successfully"),
        "expected the plain-text summary line, got: {stdout:?}"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(stdout.trim()).is_err(),
        "Table-mode stdout should not itself parse as a single JSON value: {stdout:?}"
    );
}
