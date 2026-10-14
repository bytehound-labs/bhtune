//! One serial, plant-free native OPC DA scenario, provisioned by test-opcda-yokogawa.ps1.

#![cfg(windows)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "Assertions fail an isolated, explicitly provisioned integration test"
)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Write,
    net::SocketAddr,
    path::PathBuf,
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

use bhtune_core::{ControllerDirection, ResponseLevel, TuningResultStatus};
use bhtune_db::models::{
    MvActuationKind, MvActuationStatus, RestoreStatus, SampleQuality, TimingBasis, TuneDriver,
    TuneMvActuationRow, TuneOutcome, TuneResultRow, TuneRunFilter, TuneRunRow, TuneSampleRow,
    TuneWriteRow, WriteKind, WriteReadback,
};
use serde_json::{Value, json};

const SERVER: &str = "ByteHound.BHTuneOpcDaSmoke.1";
const PREFIX: &str = "CI.Yokogawa.TC001";
const BRANCHES: [&str; 3] = ["SmokeTest", "FCS_TEST", "TC001"];
const SUFFIXES: [&str; 12] = [
    "PV", "MV", "SV", "DR", "MODE", "SH", "SL", "MSH", "MSL", "P", "I", "D",
];
const OPERATION_LIMIT: Duration = Duration::from_secs(30);
const MAX_OUTPUT_BYTES: u64 = 2 * 1024 * 1024;

struct Harness {
    root: PathBuf,
    db: PathBuf,
    config: PathBuf,
    audit: PathBuf,
    host: String,
    sequence: usize,
    session: Option<String>,
}

impl Harness {
    fn new() -> Self {
        let host = std::env::var("BHTUNE_OPCDA_SMOKE_HOST")
            .expect("run scripts/test-opcda-yokogawa.ps1 to provision the native smoke");
        let address = host
            .parse::<SocketAddr>()
            .expect("invalid fixture endpoint");
        assert_eq!(address.ip().to_string(), "127.0.0.1");
        assert!(address.port() > 0 && !matches!(address.port(), 7600 | 7602));
        let root = PathBuf::from(
            std::env::var_os("BHTUNE_OPCDA_SMOKE_DIR").expect("missing owned smoke directory"),
        )
        .canonicalize()
        .unwrap();
        let audit = PathBuf::from(
            std::env::var_os("BHTUNE_OPCDA_FIXTURE_AUDIT").expect("missing fixture audit"),
        )
        .canonicalize()
        .unwrap();
        assert!(
            audit.starts_with(&root),
            "audit must belong to the smoke directory"
        );
        assert_eq!(fs::metadata(&audit).unwrap().len(), 0);
        let db = root.join("bhtune.db");
        assert!(!db.exists(), "each smoke invocation requires fresh state");
        let catalog = root.join("templates.toml");
        File::create_new(&catalog)
            .unwrap()
            .write_all(b"template = []\n")
            .unwrap();
        let config = root.join("bhtune.toml");
        let settings = toml::toml! {
            bridge_host = (host.as_str())
            server = SERVER
            templates = (catalog.to_str().unwrap())
            allow_uncertain_quality = false
            [tuning]
            mrft_delay_secs = 0
            poll_interval_ms = 100
            timeout_secs = 120
            op_timeout_secs = 10
            restore_timeout_secs = 30
        };
        File::create_new(&config)
            .unwrap()
            .write_all(toml::to_string(&settings).unwrap().as_bytes())
            .unwrap();
        Self {
            root,
            db,
            config,
            audit,
            host,
            sequence: 0,
            session: None,
        }
    }

    fn execute(
        &mut self,
        phase: &str,
        args: &[&str],
        limit: Duration,
    ) -> std::io::Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
        self.sequence += 1;
        let base = format!("{:03}-{phase}", self.sequence);
        let stdout_path = self.root.join(format!("{base}.stdout.json"));
        let stderr_path = self.root.join(format!("{base}.stderr.log"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_bhtune"));
        for (key, _) in std::env::vars_os() {
            if key
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("BHTUNE_")
            {
                command.env_remove(key);
            }
        }
        command
            .arg("--config")
            .arg(&self.config)
            .arg("--db")
            .arg(&self.db)
            .arg("--log-dir")
            .arg(self.root.join("cli-logs"))
            .args(["--log-level", "info"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(File::create_new(&stdout_path)?)
            .stderr(File::create_new(&stderr_path)?);
        let mut child = command.spawn()?;
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) if started.elapsed() <= limit => break status,
                Ok(None) if started.elapsed() < limit => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                result => {
                    let termination = child.kill();
                    let reaped = child.wait();
                    return Err(std::io::Error::other(format!(
                        "{phase} exceeded its deadline or could not be polled: {result:?}; kill={termination:?}; reap={reaped:?}; evidence={}",
                        self.root.display()
                    )));
                }
            }
        };
        for path in [&stdout_path, &stderr_path] {
            if fs::metadata(path)?.len() > MAX_OUTPUT_BYTES {
                return Err(std::io::Error::other(format!(
                    "unbounded output in {}",
                    path.display()
                )));
            }
        }
        Ok((status, fs::read(stdout_path)?, fs::read(stderr_path)?))
    }

    fn success(&mut self, phase: &str, args: &[&str], limit: Duration) -> Value {
        let (status, stdout, stderr) = self.execute(phase, args, limit).unwrap();
        assert_eq!(
            status.code(),
            Some(0),
            "{phase}: {}\n{}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        );
        serde_json::from_slice(&stdout).expect("stdout must contain exactly one JSON value")
    }

    fn opc(&mut self, phase: &str, args: &[&str]) -> Value {
        let mut command = vec!["opc", "--output", "json"];
        command.extend_from_slice(args);
        self.success(phase, &command, OPERATION_LIMIT)
    }

    fn tune_arguments<'a>(&self, command: &'a str, pv: &'a str) -> Vec<&'a str> {
        vec![
            command,
            "--tagname",
            pv,
            "--template",
            "Yokogawa CentumVP",
            "--process-type",
            "temperature-heat-exchange",
            "--controller-type",
            "pid",
            "--driver",
            "opcda",
            "--relay-amp",
            "10",
            "--cycles-skip",
            "1",
            "--cycles-count",
            "2",
            "--noise-protection-secs",
            "0",
            "--write-pid",
            "moderate",
            "--output",
            "json",
        ]
    }

    fn save(&self, name: &str, value: &impl serde::Serialize) {
        fs::write(
            self.root.join(name),
            serde_json::to_vec_pretty(value).unwrap(),
        )
        .unwrap();
    }

    fn audit(&self) -> Vec<Value> {
        fs::read_to_string(&self.audit)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn close(&mut self) {
        if let Some(session) = self.session.clone() {
            let closed = self.opc("browse-close", &["close", &session]);
            assert_eq!(closed["session_id"], session);
            assert_eq!(closed["closed"], true);
            self.session = None;
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            match self.execute(
                "browse-cleanup",
                &["opc", "--output", "json", "close", &session],
                OPERATION_LIMIT,
            ) {
                Ok((status, _, stderr)) if !status.success() => eprintln!(
                    "Browse cleanup failed: {}",
                    String::from_utf8_lossy(&stderr)
                ),
                Err(error) => eprintln!("Browse cleanup could not run: {error}"),
                Ok(_) => {}
            }
        }
    }
}

fn tag(suffix: &str) -> String {
    format!("{PREFIX}.{suffix}")
}

fn ids(nodes: &[Value], field: &str) -> BTreeSet<String> {
    let values = nodes
        .iter()
        .map(|node| node[field].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(values.len(), nodes.len(), "duplicate {field}");
    values
}

fn assert_native_page(page: &Value, session: &str) {
    assert_eq!(page["session_id"], session);
    assert_eq!(page["organization"], "hierarchical");
    assert_eq!(page["source"], "da2");
    assert!(page["warning"].is_null(), "{page}");
}

async fn metadata_ready(harness: &Harness) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "gateway metadata deadline elapsed");
        let result = tokio::time::timeout(
            remaining.min(Duration::from_secs(2)),
            bhtune_driver::get_opcda_gateway_info(&harness.host),
        )
        .await;
        if let Ok(Ok(info)) = &result {
            assert_eq!(
                info.application_version,
                std::env::var("BHTUNE_OPCDA_SMOKE_GATEWAY_VERSION")
                    .expect("missing pinned gateway version")
            );
            harness.save(
                "gateway-info.json",
                &json!({
                    "application_version": info.application_version,
                    "compatibility_schema_version": info.compatibility_schema_version,
                    "features": info.features.iter().map(|support| json!({
                        "feature": support.feature.as_str(),
                        "min_version": support.min_version,
                        "max_version": support.max_version,
                    })).collect::<Vec<_>>(),
                }),
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "gateway metadata did not become ready: {result:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn browse(harness: &mut Harness) -> (String, BTreeSet<String>) {
    let mut page = harness.opc("browse-root", &["browse", "--page-size", "2"]);
    let session = page["session_id"].as_str().unwrap().to_owned();
    harness.session = Some(session.clone());
    let mut keys = BTreeSet::new();
    let mut parent = String::new();
    for branch in BRANCHES {
        assert_native_page(&page, &session);
        assert_eq!(page["complete"], true);
        let nodes = page["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0]["display_name"], branch);
        assert_eq!(nodes[0]["kind"], "branch");
        assert!(nodes[0]["item_id"].is_null());
        parent = nodes[0]["node_key"].as_str().unwrap().to_owned();
        assert!(!parent.is_empty() && keys.insert(parent.clone()));
        page = harness.opc(
            "browse-children",
            &[
                "browse",
                "--session-id",
                &session,
                "--parent-node-key",
                &parent,
                "--page-size",
                "2",
                "--all",
            ],
        );
    }
    assert_eq!(page["session_id"], session);
    assert_eq!(page["complete"], true);
    let pages = page["pages"]
        .as_array()
        .expect("page size two must force multiple pages");
    assert_eq!(pages.len(), 6);
    let mut leaves = vec![];
    let mut tokens = BTreeSet::new();
    for (index, page) in pages.iter().enumerate() {
        assert_native_page(page, &session);
        let nodes = page["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);
        for node in nodes {
            assert_eq!(node["kind"], "item");
            let key = node["node_key"].as_str().unwrap();
            assert!(!key.is_empty() && keys.insert(key.to_owned()));
        }
        if index + 1 < pages.len() {
            assert_eq!(page["complete"], false);
            let token = page["next_page_token"].as_str().unwrap();
            assert!(!token.is_empty() && tokens.insert(token.to_owned()));
        } else {
            assert_eq!(page["complete"], true);
            assert!(page["next_page_token"].is_null());
        }
        leaves.extend_from_slice(nodes);
    }
    let expected = SUFFIXES.into_iter().map(tag).collect::<BTreeSet<_>>();
    let item_ids = ids(&leaves, "item_id");
    assert_eq!(item_ids, expected);
    let pv = leaves
        .iter()
        .find(|node| node["display_name"] == "PV")
        .unwrap()["item_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(pv, tag("PV"));
    let read = harness.opc("browsed-pv-read", &["read", &pv]);
    assert_eq!(read["values"][0]["tag"], pv);
    assert_eq!(read["values"][0]["quality"], "good");
    assert_eq!(
        read["values"][0]["value"]
            .as_str()
            .unwrap()
            .parse::<f32>()
            .unwrap(),
        50.0
    );
    harness.close();
    let (status, stdout, stderr) = harness
        .execute(
            "browse-closed-reuse",
            &[
                "opc",
                "--output",
                "json",
                "browse",
                "--session-id",
                &session,
                "--parent-node-key",
                &parent,
            ],
            OPERATION_LIMIT,
        )
        .unwrap();
    assert_eq!(status.code(), Some(i32::from(bhtune_cli::EXIT_FAILURE)));
    assert!(
        String::from_utf8_lossy(&stdout)
            .to_ascii_lowercase()
            .contains("session")
            || String::from_utf8_lossy(&stderr)
                .to_ascii_lowercase()
                .contains("session")
    );
    (pv, item_ids)
}

fn build_index(harness: &mut Harness, pv: &str, expected: &BTreeSet<String>) {
    let fresh = harness.opc("index-fresh", &["search-index", "status"]);
    assert_eq!(fresh["state"], "not_indexed");
    assert_eq!(fresh["auto_refresh_enabled"], false);
    assert_eq!(fresh["active_generation"], 0);
    harness.opc("index-refresh", &["search-index", "refresh"]);
    let deadline = Instant::now() + Duration::from_secs(60);
    let ready = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "index build deadline elapsed");
        let status = harness.success(
            "index-status",
            &["opc", "--output", "json", "search-index", "status"],
            remaining.min(OPERATION_LIMIT),
        );
        harness.save("last-index-status.json", &status);
        assert_eq!(status["auto_refresh_enabled"], false);
        assert_ne!(status["state"], "failed", "{status}");
        if status["state"] == "ready" {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "index did not become ready: {status}"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    let generation = ready["active_generation"].as_u64().unwrap();
    assert!(generation > 0);
    assert_eq!(ready["unique_item_count"], 12);
    assert!(ready["entry_count"].as_u64().unwrap() >= 12);
    assert!(ready["database_bytes"].as_u64().unwrap() > 0);
    assert!(ready["started_at"].as_str().is_some());
    assert!(ready["completed_at"].as_str().is_some());
    assert!(ready["last_error"].is_null(), "{ready}");
    assert_eq!(ready["organization"], "hierarchical");
    assert_eq!(ready["source"], "da2");
    for (phase, query, mode) in [
        ("index-all", PREFIX, "prefix"),
        ("index-pv", pv, "exact"),
        ("index-miss", "NoSuchFixtureNativeTag", "exact"),
    ] {
        let result = harness.opc(
            phase,
            &[
                "search-index",
                "search",
                query,
                "--match-mode",
                mode,
                "--max-results",
                "32",
            ],
        );
        assert_eq!(result["has_more"], false);
        assert_eq!(result["status"]["state"], "ready");
        assert_eq!(result["status"]["active_generation"], generation);
        assert_eq!(result["status"]["auto_refresh_enabled"], false);
        let matches = result["matches"].as_array().unwrap();
        let found = ids(matches, "item_id");
        match phase {
            "index-all" => assert_eq!(&found, expected),
            "index-pv" => assert_eq!(found, BTreeSet::from([pv.to_owned()])),
            _ => assert!(found.is_empty()),
        }
        for found in matches {
            assert_eq!(found["kind"], "item");
            let suffix = found["item_id"]
                .as_str()
                .unwrap()
                .strip_prefix(&format!("{PREFIX}."))
                .unwrap();
            assert_eq!(found["display_name"], suffix);
            assert_eq!(
                found["breadcrumbs"],
                json!(["SmokeTest", "FCS_TEST", "TC001"])
            );
        }
    }
}

fn near(actual: f32, expected: f32) {
    assert!(actual.is_finite() && expected.is_finite());
    let tolerance = 0.001_f32.max(expected.abs() * 0.01);
    assert!(
        (actual - expected).abs() <= tolerance,
        "expected {expected}, got {actual}"
    );
}

async fn assert_tune(harness: &Harness, summary: &Value, pv: &str) -> [f32; 3] {
    assert_eq!(summary["outcome"], "completed", "{summary}");
    assert_eq!(summary["write_back"], "written", "{summary}");
    assert_eq!(summary["write_back_response_level"], "moderate");
    let pool = bhtune_db::connect_read_only(&harness.db).await.unwrap();
    assert_eq!(
        TuneRunRow::count(&pool, &TuneRunFilter::default())
            .await
            .unwrap(),
        1
    );
    let id = summary["run_id"].as_i64().unwrap();
    let run = TuneRunRow::get(&pool, id).await.unwrap().unwrap();
    harness.save("persisted-run.json", &run);
    assert_eq!(run.driver, TuneDriver::Opcda);
    assert_eq!(run.outcome, TuneOutcome::Completed);
    assert_eq!(run.opc_server.as_deref(), Some(SERVER));
    assert_eq!(run.bridge_host.as_deref(), Some(harness.host.as_str()));
    assert!(run.completed_at.is_some() && run.failure_reason.is_none());
    assert_eq!(run.restore_status, Some(RestoreStatus::Confirmed));
    assert!(run.restore_detail.is_none());
    assert_eq!(run.template.name, "Yokogawa CentumVP");
    assert_eq!(
        serde_json::to_value(run.template.pid_rounding).unwrap(),
        json!({"kind": "decimal_places", "digits": 1})
    );
    assert_eq!(run.tags.process_variable, pv);
    assert_eq!(run.tags.manipulated_variable, tag("MV"));
    for (actual, suffix) in [
        (&run.tags.setpoint_variable, "SV"),
        (&run.tags.controller_mode, "MODE"),
        (&run.tags.proportional_constant, "P"),
        (&run.tags.integral_constant, "I"),
        (&run.tags.derivative_constant, "D"),
    ] {
        assert_eq!(actual.as_deref(), Some(tag(suffix).as_str()));
    }
    let initial = run.initial_readings.as_ref().unwrap();
    assert_eq!(initial.controller_direction, ControllerDirection::Reverse);
    assert_eq!(initial.pv_ini, 50.0);
    assert_eq!(initial.mv_ini, 50.0);
    assert_eq!(initial.mode_raw.as_deref(), Some("AUT"));
    assert_eq!(initial.setpoint_ini, Some(50.0));
    assert_eq!((initial.pv_range_low, initial.pv_range_high), (0.0, 100.0));
    assert_eq!((initial.mv_range_low, initial.mv_range_high), (0.0, 100.0));
    assert_eq!(
        run.timing_metrics.unwrap().basis,
        TimingBasis::LiveMonotonic
    );
    let samples = TuneSampleRow::list_for_run(&pool, id).await.unwrap();
    assert!(samples.len() >= 10);
    assert!(
        samples
            .iter()
            .all(|sample| sample.pv_quality == SampleQuality::Good && sample.sample.pv.is_finite())
    );
    let actuations = TuneMvActuationRow::list_for_run(&pool, id).await.unwrap();
    harness.save("mv-actuations.json", &actuations);
    assert!(actuations.len() >= 4);
    let restore = actuations.last().unwrap();
    // A verified final relay snapback can be handed to restore without a duplicate MV write.
    assert!(matches!(
        restore.kind,
        MvActuationKind::Relay | MvActuationKind::Restore
    ));
    assert_eq!(restore.target_mv, 50.0);
    for (index, actuation) in actuations.iter().enumerate() {
        if actuation.status == MvActuationStatus::Superseded {
            assert_eq!(actuation.kind, MvActuationKind::Relay);
            assert_eq!(actuation.target_mv, 50.0);
            if index + 1 < actuations.len() {
                assert_eq!(
                    index + 2,
                    actuations.len(),
                    "only final snapback may be superseded"
                );
                assert_eq!(restore.kind, MvActuationKind::Restore);
                continue;
            }
            assert!(
                actuation
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains("adopted and confirmed"))
            );
        } else {
            assert_eq!(
                actuation.status,
                MvActuationStatus::Confirmed,
                "{actuation:?}"
            );
        }
        assert_eq!(actuation.readback_quality, Some(SampleQuality::Good));
        near(actuation.readback_mv.unwrap(), actuation.target_mv);
        assert!(actuation.last_checked_at.unwrap() <= actuation.confirmation_due_at);
    }
    let results = TuneResultRow::list_for_run(&pool, id).await.unwrap();
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(
        |result| result.status == TuningResultStatus::Valid && result.invalid_reason.is_none()
    ));
    let result = results
        .iter()
        .find(|result| result.response_level == ResponseLevel::Moderate)
        .unwrap();
    let converted = [
        100.0 / result.kp.unwrap(),
        result.ti_minutes.unwrap() * 60.0,
        result.td_minutes.unwrap() * 60.0,
    ];
    for (actual, expected) in [
        result.proportional.unwrap(),
        result.integral.unwrap(),
        result.derivative.unwrap(),
    ]
    .into_iter()
    .zip(converted)
    {
        near(actual, expected);
    }
    let targets = converted.map(|value| ((f64::from(value) * 10.0).round() / 10.0) as f32);
    assert!(
        targets
            .iter()
            .all(|value| value.is_finite() && *value > 0.0)
    );
    let previews = summary["controller_pid"].as_array().unwrap();
    let preview = previews
        .iter()
        .find(|preview| preview["response_level"] == "moderate")
        .unwrap();
    for (field, target) in ["proportional", "integral", "derivative"]
        .into_iter()
        .zip(targets)
    {
        near(
            preview["values"][field]["value"].as_f64().unwrap() as f32,
            target,
        );
        assert_eq!(preview["values"][field]["display"], format!("{target:.1}"));
    }
    let writes = TuneWriteRow::list_for_run(&pool, id).await.unwrap();
    assert_eq!(writes.len(), 1);
    let write = &writes[0];
    assert_eq!(write.kind, WriteKind::Write);
    assert_eq!(write.response_level, ResponseLevel::Moderate);
    assert!(write.success, "{write:?}");
    assert_eq!(
        write.previous,
        Some(WriteReadback {
            proportional: 120.0,
            integral: 30.0,
            derivative: 5.0
        })
    );
    assert!(
        write.error_message.is_none()
            && write.rollback_state.is_none()
            && write.rollback_error.is_none()
    );
    for (written, readback, target) in [
        (
            write.proportional_written,
            write.proportional_readback,
            targets[0],
        ),
        (write.integral_written, write.integral_readback, targets[1]),
        (
            write.derivative_written,
            write.derivative_readback,
            targets[2],
        ),
    ] {
        assert_eq!(written, Some(target));
        near(readback.unwrap(), target);
    }
    harness.save("pid-oracle.json", &json!({"raw_units": converted, "rounded_targets": targets, "sample_count": samples.len(), "write_id": write.id}));
    pool.close().await;
    targets
}

fn assert_final(harness: &mut Harness, targets: [f32; 3]) {
    let names = ["P", "I", "D", "MV", "MODE", "SV"].map(tag);
    let mut args = vec!["read"];
    args.extend(names.iter().map(String::as_str));
    let final_read = harness.opc("final-read", &args);
    let values = final_read["values"].as_array().unwrap();
    assert_eq!(values.len(), names.len());
    let values = values
        .iter()
        .map(|value| {
            assert_eq!(value["quality"], "good");
            (
                value["tag"].as_str().unwrap(),
                value["value"].as_str().unwrap(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(values.len(), names.len());
    for (index, target) in targets.into_iter().enumerate() {
        near(values[names[index].as_str()].parse().unwrap(), target);
    }
    assert_eq!(values[names[3].as_str()].parse::<f32>().unwrap(), 50.0);
    assert_eq!(values[names[4].as_str()], "AUT");
    assert_eq!(values[names[5].as_str()].parse::<f32>().unwrap(), 50.0);
    let audit = harness.audit();
    assert!(!audit.is_empty() && audit.len() <= 256);
    assert_eq!(audit[0]["tag"], tag("MODE"));
    assert_eq!(audit[0]["mode_before"], "AUT");
    assert_eq!(audit[0]["value"], "MAN");
    let pid = audit
        .iter()
        .filter(|row| {
            ["P", "I", "D"]
                .into_iter()
                .any(|suffix| row["tag"] == tag(suffix))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        pid.len(),
        3,
        "there must be exactly one P/I/D write sequence"
    );
    for ((row, suffix), target) in pid.iter().zip(["P", "I", "D"]).zip(targets) {
        assert_eq!(row["tag"], tag(suffix));
        near(row["value"].as_f64().unwrap() as f32, target);
    }
    let mv = audit
        .iter()
        .filter(|row| row["tag"] == tag("MV"))
        .collect::<Vec<_>>();
    assert!(mv.len() >= 4);
    assert!(mv.iter().all(|row| row["mode_before"] == "MAN"));
    assert_eq!(mv.last().unwrap()["value"], 50.0);
    let restored_mode = audit
        .iter()
        .position(|row| row["tag"] == tag("MODE") && row["value"] == "AUT")
        .unwrap();
    assert!(
        mv.last().unwrap()["sequence"].as_u64().unwrap()
            < audit[restored_mode]["sequence"].as_u64().unwrap()
    );
    assert!(
        audit
            .iter()
            .any(|row| row["tag"] == tag("SV") && row["value"] == 50.0)
    );
    for (index, row) in audit.iter().enumerate() {
        assert_eq!(row["sequence"], index + 1);
        assert!(
            ["MV", "SV", "MODE", "P", "I", "D"]
                .into_iter()
                .any(|suffix| row["tag"] == tag(suffix))
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the isolated x86 COM fixture and checksum-pinned official gateway"]
async fn native_yokogawa_preflight_browse_index_tune_and_pid_write() {
    let mut harness = Harness::new();
    metadata_ready(&harness).await;
    let initial_pv = tag("PV");
    let args = harness.tune_arguments("check", &initial_pv);
    let check = harness.success("preflight", &args, OPERATION_LIMIT);
    assert_eq!(check["status"], "pass", "{check}");
    let reads = check["tag_reads"].as_array().unwrap();
    assert_eq!(ids(reads, "tag"), SUFFIXES.into_iter().map(tag).collect());
    assert!(
        reads
            .iter()
            .all(|read| read["quality"] == "Good" && read["status"] == "pass")
    );
    assert!(
        !harness.db.exists(),
        "preflight must not create its database"
    );
    assert!(harness.audit().is_empty(), "preflight must not write tags");
    let (pv, browsed) = browse(&mut harness);
    build_index(&mut harness, &pv, &browsed);
    assert!(
        harness.audit().is_empty(),
        "browse/read/index phases must not write tags"
    );
    let pool = bhtune_db::connect_read_only(&harness.db).await.unwrap();
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM tune_runs), (SELECT COUNT(*) FROM tune_writes)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(counts, (0, 0));
    pool.close().await;
    let mut args = harness.tune_arguments("tune", &pv);
    args.push("--yes");
    let summary = harness.success("tune", &args, Duration::from_secs(180));
    let targets = assert_tune(&harness, &summary, &pv).await;
    assert_final(&mut harness, targets);
    harness.save("smoke-result.json", &json!({"passed": true, "run_id": summary["run_id"], "fixture": SERVER, "browsed_items": browsed, "pid_targets": targets}));
}
