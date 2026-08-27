//! Fully automated, Linux-CI-friendly end-to-end coverage for a real `bhtune tune` run
//! against the in-process simulator driver (`e2e-simulator`): spawns the actual compiled
//! `bhtune` binary (not an in-process call), lets it complete a full MRFT test, then opens
//! the resulting SQLite database directly and asserts the *calculated tuning results* are
//! sane -- not just that rows exist.
//!
//! This closes a real gap no earlier test covered. `json_output_contract.rs`/
//! `ctrlc_abort.rs` are genuine subprocess tests but only check the JSON summary's
//! shape/exit code, which never includes calculated Kp/Ti/Td at all (see
//! `commands::tune::print_summary`). `commands::tune`'s own in-process
//! `a_full_simulator_tune_completes_and_persists_results` test checks row *presence/counts*
//! only. Neither ever asserted an actual numeric result was correct.
//!
//! Writing this test is what surfaced a real production bug: `measure_oscillation`
//! (`bhtune-core`'s `tuning_math.rs`) computed the relay oscillation period via
//! `chrono::Duration::num_seconds()` *unconditionally*, truncating to whole seconds even
//! though the surrounding `TuningMathCompat.replicate_period_truncation_bug` flag's own doc
//! comment says the *default* path should preserve full precision. Every existing unit test
//! for that function used whole-second switch-time offsets, so the truncation was always
//! lossless there and the gap went unnoticed. This test's simulator runs complete in tens of
//! milliseconds of simulated relay-switch spacing (by design -- see `fast_simulator_args()`
//! in `commands::tune`), so it hit the bug immediately: `ti_minutes`/`td_minutes` came back
//! as exactly `0.0` even for PI/PID controller types with a nonzero `C2`/`C3` matrix entry,
//! which is what `results_are_sane_and_correctly_ordered` below directly guards against.
//!
//! All three matrix cases use `direction=reverse`: empirically confirmed (by hand, before
//! writing this test) to be the only direction that produces a valid relay oscillation
//! against this driver's fixed `sim_gain=1.0`/`sim_tau=0.01`/`sim_dead_time=0.025`
//! parameters -- `direction=direct` rails the simulated process out instead of oscillating,
//! which is a genuine control-theory sign-mismatch (relay pushes the same way the process is
//! already moving), not a bug. `process_type`/`controller_type` are varied freely across the
//! matrix since neither affects the simulated plant dynamics, only which tuning-constant
//! matrix cell (`bhtune-core`'s `constants.rs`) is looked up.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use bhtune_core::constants::ResponseLevel;
use bhtune_core::controller_type::ControllerType;
use bhtune_core::process_type::ProcessType;
use bhtune_db::models::{TuneOutcome, TuneResultRow, TuneRunRow, TuneSampleRow};

/// Spawns `bhtune tune` against a fresh temp DB with the fast-completing simulator
/// parameters (see `commands::tune::fast_simulator_args()`), fixed at `direction=reverse`,
/// varying only `--process-type`/`--controller-type`. Returns the exit code, stdout, and the
/// path to the SQLite database the run wrote to (the caller's `db_dir`/`log_dir` tempdirs
/// must outlive this call, hence they're passed in rather than created here).
fn run_fast_simulator_tune(
    db_path: &Path,
    log_dir: &Path,
    process_type_arg: &str,
    controller_type_arg: &str,
) -> (Option<i32>, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_bhtune"))
        .arg("--db")
        .arg(db_path)
        .arg("--log-dir")
        .arg(log_dir)
        .args([
            "tune",
            "--tagname",
            "ignored-for-simulator",
            "--template",
            "Yokogawa CentumVP",
            "--process-type",
            process_type_arg,
            "--controller-type",
            controller_type_arg,
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
            "--timeout-secs",
            "10",
            "--notes",
            "e2e-simulator-test",
            "--output",
            "json",
        ])
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

/// Runs one matrix case end-to-end and asserts the full set of invariants a real tune's
/// results must satisfy, regardless of process/controller type: clean completion, exactly
/// one row per [`ResponseLevel`], strictly-decreasing `kp` from Aggressive to Sluggish, a
/// non-empty sample trail, and -- the specific regression this file exists to guard --
/// `ti_minutes`/`td_minutes` that are identical across all three response levels and match
/// the caller's expectation of whether they should be exactly zero (P-only has no integral
/// or derivative term) or genuinely nonzero (PI/PID do, and must not silently collapse to
/// zero the way the pre-fix truncation bug made them).
async fn assert_matrix_case(
    process_type_arg: &str,
    controller_type_arg: &str,
    expected_process_type: ProcessType,
    expected_controller_type: ControllerType,
    expect_nonzero_ti: bool,
    expect_nonzero_td: bool,
) {
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("bhtune.db");
    // See `ctrlc_abort.rs`/`json_output_contract.rs`'s identical comment: without an
    // explicit temp log dir, logging setup would resolve the real platform default log
    // directory using this test process's inherited environment, writing real files
    // under the developer/CI machine's actual home directory as a side effect of running
    // this test.
    let log_dir = tempfile::tempdir().unwrap();

    let (exit_code, stdout, stderr) = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::task::spawn_blocking({
            let db_path = db_path.clone();
            let log_dir = log_dir.path().to_path_buf();
            let process_type_arg = process_type_arg.to_string();
            let controller_type_arg = controller_type_arg.to_string();
            move || {
                run_fast_simulator_tune(&db_path, &log_dir, &process_type_arg, &controller_type_arg)
            }
        }),
    )
    .await
    .expect("bhtune did not exit within 30s")
    .expect("joining the spawn_blocking task panicked");

    assert_eq!(
        exit_code,
        Some(bhtune_cli::EXIT_SUCCESS as i32),
        "expected a clean completion; stderr: {stderr}"
    );

    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("stdout was not exactly one parseable JSON value: {e}\nstdout was: {stdout:?}")
    });
    assert_eq!(json["outcome"], "completed", "full JSON was: {json}");
    let run_id = json["run_id"]
        .as_i64()
        .expect("run_id must be present and an integer in the JSON summary");

    let pool = bhtune_db::connect(&db_path)
        .await
        .expect("failed to open the database the subprocess just wrote to");

    let run = TuneRunRow::get(&pool, run_id)
        .await
        .expect("querying the run row failed")
        .expect("the run the subprocess just reported by id must exist in the database");
    assert_eq!(run.outcome, TuneOutcome::Completed);
    assert_eq!(run.config.process_type, expected_process_type);
    assert_eq!(run.config.controller_type, expected_controller_type);

    let results = TuneResultRow::list_for_run(&pool, run_id)
        .await
        .expect("querying tune_results failed");
    assert_eq!(
        results.len(),
        3,
        "a completed run must have exactly one result row per ResponseLevel, got: {results:?}"
    );

    let by_level = |level: ResponseLevel| {
        results
            .iter()
            .find(|r| r.response_level == level)
            .unwrap_or_else(|| panic!("missing a {level:?} result row, got: {results:?}"))
    };
    let aggressive = by_level(ResponseLevel::Aggressive);
    let moderate = by_level(ResponseLevel::Moderate);
    let sluggish = by_level(ResponseLevel::Sluggish);

    assert!(aggressive.kp > 0.0, "kp must be positive: {aggressive:?}");
    assert!(moderate.kp > 0.0, "kp must be positive: {moderate:?}");
    assert!(sluggish.kp > 0.0, "kp must be positive: {sluggish:?}");
    assert!(
        aggressive.kp > moderate.kp && moderate.kp > sluggish.kp,
        "kp must strictly decrease Aggressive > Moderate > Sluggish, got: \
         aggressive={}, moderate={}, sluggish={}",
        aggressive.kp,
        moderate.kp,
        sluggish.kp
    );

    // ti_minutes/td_minutes are response-level-invariant by design (C2/C3/BETA are indexed
    // only by [process_type][controller_type], not by response level) -- proven already by
    // `calculate_tuning_result_ti_td_invariant_across_response_levels` in bhtune-core, and
    // re-asserted here as a real end-to-end property rather than assumed.
    assert_eq!(aggressive.ti_minutes, moderate.ti_minutes);
    assert_eq!(moderate.ti_minutes, sluggish.ti_minutes);
    assert_eq!(aggressive.td_minutes, moderate.td_minutes);
    assert_eq!(moderate.td_minutes, sluggish.td_minutes);

    // The regression this file exists to catch: before the `measure_oscillation` fix,
    // ti_minutes/td_minutes were silently exactly 0.0 for *every* case, including PI/PID,
    // because the sub-second relay period truncated to whole seconds. A genuine, real
    // relay oscillation happened (proven by the non-empty sample trail asserted below);
    // the bug was purely in the period arithmetic that turns switch timestamps into
    // ti_minutes/td_minutes.
    if expect_nonzero_ti {
        assert!(
            aggressive.ti_minutes > 0.0,
            "ti_minutes must be genuinely nonzero for a PI/PID controller type, got \
             exactly {} -- this is the sub-second period-truncation bug if it recurs",
            aggressive.ti_minutes
        );
    } else {
        assert_eq!(
            aggressive.ti_minutes, 0.0,
            "ti_minutes must be exactly zero for a P-only controller type"
        );
    }
    if expect_nonzero_td {
        assert!(
            aggressive.td_minutes > 0.0,
            "td_minutes must be genuinely nonzero for a PID controller type, got \
             exactly {} -- this is the sub-second period-truncation bug if it recurs",
            aggressive.td_minutes
        );
    } else {
        assert_eq!(
            aggressive.td_minutes, 0.0,
            "td_minutes must be exactly zero for a non-PID controller type"
        );
    }

    let samples = TuneSampleRow::list_for_run(&pool, run_id)
        .await
        .expect("querying tune_samples failed");
    assert!(
        !samples.is_empty(),
        "a completed run must have recorded at least one sample tick"
    );

    pool.close().await;
}

/// Flow process type, PI controller: the simplest matrix case, and the one manually
/// verified by hand (via a real subprocess run + direct `sqlite3` inspection) before this
/// test was written, to confirm the expected shape before hardcoding it.
#[tokio::test]
async fn flow_pi_reverse_produces_sane_ordered_results() {
    assert_matrix_case(
        "flow",
        "pi",
        ProcessType::Flow,
        ControllerType::Pi,
        true,  // PI has a nonzero integral term
        false, // PI has no derivative term
    )
    .await;
}

/// Temperature (heat exchange) process type, PID controller -- the only process type/
/// controller-type combination in this matrix that exercises a nonzero derivative term
/// (PID is only ever offered for the two Temperature process types, per
/// `ControllerType::is_allowed_for`).
#[tokio::test]
async fn temperature_heat_exchange_pid_reverse_produces_sane_ordered_results() {
    assert_matrix_case(
        "temperature-heat-exchange",
        "pid",
        ProcessType::TemperatureHeatExchange,
        ControllerType::Pid,
        true, // PID has a nonzero integral term
        true, // PID has a nonzero derivative term
    )
    .await;
}

/// Level process type, P-only controller: proves the zero-integral/zero-derivative case is
/// still handled correctly (i.e. that fixing the truncation bug didn't turn a
/// legitimately-zero `ti_minutes`/`td_minutes` into some other, still-wrong nonzero value).
#[tokio::test]
async fn level_p_reverse_produces_sane_ordered_results() {
    assert_matrix_case(
        "level",
        "p",
        ProcessType::Level,
        ControllerType::P,
        false, // P-only has no integral term
        false, // P-only has no derivative term
    )
    .await;
}
