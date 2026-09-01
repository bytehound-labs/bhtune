//! Fully automated, Linux-CI-friendly end-to-end coverage for real `bhtune tune` runs
//! against the in-process simulator driver (`e2e-simulator`): spawns the actual compiled
//! `bhtune` binary (not an in-process call), lets each full MRFT test complete, then opens
//! the resulting SQLite database directly and compares every persisted PID value with a
//! reviewed numeric baseline.
//!
//! This is the numeric envelope test for the production CLI path. It covers argument parsing,
//! simulator construction, MRFT orchestration, tuning math, result persistence, and process
//! exit behavior without adding browser rendering, SSE delivery, or selector timing to the
//! numeric oracle. The Playwright tune test remains responsible for proving that the server and
//! SPA can start and render a tune, while `bhtune-core`'s golden replay remains the strict,
//! deterministic legacy-parity oracle.
//!
//! The matrix uses Flow/PI, Temperature (Heat Exchange)/PID, and Level/P cases. All cases use
//! `direction=reverse`, which is the direction that produces a valid oscillation against the
//! simulator's fixed process gain. The simulator advances both its FOPDT process and the MRFT
//! timestamp by the same exact 5 ms step per PV read (`sim_tau=0.01`,
//! `sim_dead_time=0.025`), so host scheduling can lengthen the subprocess's wall-clock runtime
//! but cannot change its calculated PID values.
//!
//! The reviewed baselines below use that fixed simulator time domain. All nonzero values use
//! tight absolute-plus-relative tolerances for cross-platform floating-point math and SQLite
//! round trips; there is no scheduler-jitter allowance. Fields that are mathematically absent
//! for a controller type must remain exactly zero.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use bhtune_core::constants::ResponseLevel;
use bhtune_core::controller_type::ControllerType;
use bhtune_core::process_type::ProcessType;
use bhtune_db::models::{TuneOutcome, TuneResultRow, TuneRunRow, TuneSampleRow};

const AMPLITUDE_ABSOLUTE_TOLERANCE: f32 = 1e-3;
const AMPLITUDE_RELATIVE_TOLERANCE: f32 = 1e-4;
const PERIOD_ABSOLUTE_TOLERANCE: f32 = 1e-7;
const PERIOD_RELATIVE_TOLERANCE: f32 = 1e-4;
const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy)]
struct ExpectedResult {
    response_level: ResponseLevel,
    kp: f32,
    ti_minutes: f32,
    td_minutes: f32,
    proportional: f32,
    integral: f32,
    derivative: f32,
}

#[derive(Debug, Clone, Copy)]
struct SimulatorScenario {
    name: &'static str,
    process_type_arg: &'static str,
    controller_type_arg: &'static str,
    process_type: ProcessType,
    controller_type: ControllerType,
    expected_results: [ExpectedResult; 3],
}

/// Spawns `bhtune tune` against a fresh temp DB with the deterministic, fast-completing simulator
/// parameters. The process/controller type changes which tuning-constant matrix cell is used;
/// the simulated plant and all timing parameters stay identical across scenarios.
fn run_simulator_tune(
    db_path: &Path,
    log_dir: &Path,
    process_type_arg: &str,
    controller_type_arg: &str,
) -> (Option<i32>, String, String) {
    let config_path = db_path.with_file_name("bhtune.toml");
    std::fs::write(
        &config_path,
        "[tuning]\npoll_interval_ms = 5\ntimeout_secs = 30\n",
    )
    .expect("failed to write test configuration");
    let output = Command::new(env!("CARGO_BIN_EXE_bhtune"))
        .arg("--db")
        .arg(db_path)
        .arg("--config")
        .arg(&config_path)
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

#[allow(clippy::excessive_precision)] // Preserve the recorded f32 baseline values verbatim.
fn simulator_scenarios() -> [SimulatorScenario; 3] {
    [
        SimulatorScenario {
            name: "Flow / PI / Reverse",
            process_type_arg: "flow",
            controller_type_arg: "pi",
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            expected_results: [
                ExpectedResult {
                    response_level: ResponseLevel::Aggressive,
                    kp: 0.595620036,
                    ti_minutes: 0.000441333,
                    td_minutes: 0.0,
                    proportional: 167.892272949,
                    integral: 0.026479999,
                    derivative: 0.0,
                },
                ExpectedResult {
                    response_level: ResponseLevel::Moderate,
                    kp: 0.398840874,
                    ti_minutes: 0.000441333,
                    td_minutes: 0.0,
                    proportional: 250.726562500,
                    integral: 0.026479999,
                    derivative: 0.0,
                },
                ExpectedResult {
                    response_level: ResponseLevel::Sluggish,
                    kp: 0.298470318,
                    ti_minutes: 0.000441333,
                    td_minutes: 0.0,
                    proportional: 335.041687012,
                    integral: 0.026479999,
                    derivative: 0.0,
                },
            ],
        },
        SimulatorScenario {
            name: "Temperature Heat Exchange / PID / Reverse",
            process_type_arg: "temperature-heat-exchange",
            controller_type_arg: "pid",
            process_type: ProcessType::TemperatureHeatExchange,
            controller_type: ControllerType::Pid,
            expected_results: [
                ExpectedResult {
                    response_level: ResponseLevel::Aggressive,
                    kp: 0.449071199,
                    ti_minutes: 0.000320833,
                    td_minutes: 0.000105000,
                    proportional: 222.681838989,
                    integral: 0.019250000,
                    derivative: 0.006300000,
                },
                ExpectedResult {
                    response_level: ResponseLevel::Moderate,
                    kp: 0.300282568,
                    ti_minutes: 0.000320833,
                    td_minutes: 0.000105000,
                    proportional: 333.019653320,
                    integral: 0.019250000,
                    derivative: 0.006300000,
                },
                ExpectedResult {
                    response_level: ResponseLevel::Sluggish,
                    kp: 0.224535599,
                    ti_minutes: 0.000320833,
                    td_minutes: 0.000105000,
                    proportional: 445.363677979,
                    integral: 0.019250000,
                    derivative: 0.006300000,
                },
            ],
        },
        SimulatorScenario {
            name: "Level / P / Reverse",
            process_type_arg: "level",
            controller_type_arg: "p",
            process_type: ProcessType::Level,
            controller_type: ControllerType::P,
            expected_results: [
                ExpectedResult {
                    response_level: ResponseLevel::Aggressive,
                    kp: 0.450423837,
                    ti_minutes: 0.0,
                    td_minutes: 0.0,
                    proportional: 222.013122559,
                    integral: 0.0,
                    derivative: 0.0,
                },
                ExpectedResult {
                    response_level: ResponseLevel::Moderate,
                    kp: 0.300282568,
                    ti_minutes: 0.0,
                    td_minutes: 0.0,
                    proportional: 333.019653320,
                    integral: 0.0,
                    derivative: 0.0,
                },
                ExpectedResult {
                    response_level: ResponseLevel::Sluggish,
                    kp: 0.225888222,
                    ti_minutes: 0.0,
                    td_minutes: 0.0,
                    proportional: 442.696838379,
                    integral: 0.0,
                    derivative: 0.0,
                },
            ],
        },
    ]
}

fn assert_amplitude_matches(
    scenario: &str,
    response_level: ResponseLevel,
    field: &str,
    actual: f32,
    expected: f32,
) {
    let tolerance = AMPLITUDE_ABSOLUTE_TOLERANCE + expected.abs() * AMPLITUDE_RELATIVE_TOLERANCE;
    assert!(
        (actual - expected).abs() <= tolerance,
        "{scenario} {response_level:?} {field}: expected {expected:.9}, got {actual:.9}, \
         allowed tolerance {tolerance:.9} (absolute {AMPLITUDE_ABSOLUTE_TOLERANCE:.9}, \
         relative {AMPLITUDE_RELATIVE_TOLERANCE:.6})"
    );
}

fn assert_period_matches(
    scenario: &str,
    response_level: ResponseLevel,
    field: &str,
    actual: f32,
    expected: f32,
) {
    if expected == 0.0 {
        assert_eq!(
            actual, 0.0,
            "{scenario} {response_level:?} {field}: expected exact zero, got {actual:.9}"
        );
        return;
    }

    let tolerance = PERIOD_ABSOLUTE_TOLERANCE + expected.abs() * PERIOD_RELATIVE_TOLERANCE;
    assert!(
        (actual - expected).abs() <= tolerance,
        "{scenario} {response_level:?} {field}: expected {expected:.9}, got {actual:.9}, \
         allowed tolerance {tolerance:.9} (absolute {PERIOD_ABSOLUTE_TOLERANCE:.9}, \
         relative {PERIOD_RELATIVE_TOLERANCE:.2})"
    );
}

fn assert_result_matches(scenario: &str, actual: &TuneResultRow, expected: ExpectedResult) {
    assert_eq!(
        actual.response_level, expected.response_level,
        "{scenario}: result row has the wrong response level"
    );
    assert_amplitude_matches(
        scenario,
        expected.response_level,
        "kp",
        actual.kp,
        expected.kp,
    );
    assert_period_matches(
        scenario,
        expected.response_level,
        "ti_minutes",
        actual.ti_minutes,
        expected.ti_minutes,
    );
    assert_period_matches(
        scenario,
        expected.response_level,
        "td_minutes",
        actual.td_minutes,
        expected.td_minutes,
    );
    assert_amplitude_matches(
        scenario,
        expected.response_level,
        "proportional",
        actual.proportional,
        expected.proportional,
    );
    assert_period_matches(
        scenario,
        expected.response_level,
        "integral",
        actual.integral,
        expected.integral,
    );
    assert_period_matches(
        scenario,
        expected.response_level,
        "derivative",
        actual.derivative,
        expected.derivative,
    );
}

/// Runs one matrix case end-to-end and asserts lifecycle, identity, ordering, response-level
/// invariance, sample persistence, and the reviewed numeric baseline for every result field.
async fn assert_matrix_case(scenario: SimulatorScenario) {
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("bhtune.db");
    // Without an explicit temp log dir, logging setup would resolve the real platform default
    // log directory using this test process's inherited environment.
    let log_dir = tempfile::tempdir().unwrap();

    let (exit_code, stdout, stderr) = tokio::time::timeout(
        SUBPROCESS_TIMEOUT,
        tokio::task::spawn_blocking({
            let db_path = db_path.clone();
            let log_dir = log_dir.path().to_path_buf();
            move || {
                run_simulator_tune(
                    &db_path,
                    &log_dir,
                    scenario.process_type_arg,
                    scenario.controller_type_arg,
                )
            }
        }),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "{} did not exit within {}s",
            scenario.name,
            SUBPROCESS_TIMEOUT.as_secs()
        )
    })
    .expect("joining the spawn_blocking task panicked");

    assert_eq!(
        exit_code,
        Some(bhtune_cli::EXIT_SUCCESS as i32),
        "{}: expected a clean completion; stderr: {stderr}",
        scenario.name
    );

    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "{}: stdout was not exactly one parseable JSON value: {e}\nstdout: {stdout:?}",
            scenario.name
        )
    });
    assert_eq!(
        json["outcome"], "completed",
        "{}: full JSON was: {json}",
        scenario.name
    );
    let run_id = json["run_id"].as_i64().unwrap_or_else(|| {
        panic!(
            "{}: run_id must be present and an integer in the JSON summary",
            scenario.name
        )
    });

    let pool = bhtune_db::connect(&db_path)
        .await
        .unwrap_or_else(|e| panic!("{}: failed to open the test database: {e}", scenario.name));

    let run = TuneRunRow::get(&pool, run_id)
        .await
        .unwrap_or_else(|e| panic!("{}: querying the run row failed: {e}", scenario.name))
        .unwrap_or_else(|| {
            panic!(
                "{}: the run reported by the subprocess must exist in the database",
                scenario.name
            )
        });
    assert_eq!(run.outcome, TuneOutcome::Completed);
    assert_eq!(run.config.process_type, scenario.process_type);
    assert_eq!(run.config.controller_type, scenario.controller_type);

    let results = TuneResultRow::list_for_run(&pool, run_id)
        .await
        .unwrap_or_else(|e| panic!("{}: querying tune_results failed: {e}", scenario.name));
    assert_eq!(
        results.len(),
        3,
        "{}: a completed run must have exactly one result row per ResponseLevel, got: {results:?}",
        scenario.name
    );

    let by_level = |level: ResponseLevel| {
        results
            .iter()
            .find(|result| result.response_level == level)
            .unwrap_or_else(|| {
                panic!(
                    "{}: missing a {level:?} result row, got: {results:?}",
                    scenario.name
                )
            })
    };
    let aggressive = by_level(ResponseLevel::Aggressive);
    let moderate = by_level(ResponseLevel::Moderate);
    let sluggish = by_level(ResponseLevel::Sluggish);

    assert!(
        aggressive.kp > 0.0,
        "{}: kp must be positive",
        scenario.name
    );
    assert!(moderate.kp > 0.0, "{}: kp must be positive", scenario.name);
    assert!(sluggish.kp > 0.0, "{}: kp must be positive", scenario.name);
    assert!(
        aggressive.kp > moderate.kp && moderate.kp > sluggish.kp,
        "{}: kp must strictly decrease Aggressive > Moderate > Sluggish, got: \
         aggressive={}, moderate={}, sluggish={}",
        scenario.name,
        aggressive.kp,
        moderate.kp,
        sluggish.kp
    );

    // Ti/Td are response-level-invariant by design: their constants are indexed only by
    // process/controller type, not response level.
    assert_eq!(
        aggressive.ti_minutes, moderate.ti_minutes,
        "{}: ti_minutes must be response-level invariant",
        scenario.name
    );
    assert_eq!(
        moderate.ti_minutes, sluggish.ti_minutes,
        "{}: ti_minutes must be response-level invariant",
        scenario.name
    );
    assert_eq!(
        aggressive.td_minutes, moderate.td_minutes,
        "{}: td_minutes must be response-level invariant",
        scenario.name
    );
    assert_eq!(
        moderate.td_minutes, sluggish.td_minutes,
        "{}: td_minutes must be response-level invariant",
        scenario.name
    );

    for expected in scenario.expected_results {
        let actual = by_level(expected.response_level);
        assert_result_matches(scenario.name, actual, expected);
    }

    let samples = TuneSampleRow::list_for_run(&pool, run_id)
        .await
        .unwrap_or_else(|e| panic!("{}: querying tune_samples failed: {e}", scenario.name));
    assert!(
        !samples.is_empty(),
        "{}: a completed run must have recorded at least one sample tick",
        scenario.name
    );
    assert_eq!(
        (samples[0].sample.time - run.started_at).num_milliseconds(),
        5,
        "{}: the first persisted simulator sample must be one process step after the run start",
        scenario.name
    );
    for pair in samples.windows(2) {
        assert_eq!(
            (pair[1].sample.time - pair[0].sample.time).num_milliseconds(),
            5,
            "{}: persisted simulator samples must use the fixed 5 ms process step",
            scenario.name
        );
    }

    pool.close().await;
}

/// Runs the numeric matrix serially to keep CI resource use predictable. The simulator's fixed
/// time domain makes the calculated values independent of host scheduling, but one test also
/// keeps subprocess startup and SQLite activity from competing unnecessarily.
#[tokio::test]
async fn simulator_tunes_match_reviewed_numeric_baselines() {
    for scenario in simulator_scenarios() {
        assert_matrix_case(scenario).await;
    }
}
