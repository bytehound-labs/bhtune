//! Command-line argument definitions (`clap` derive) and the small wrapper enums that adapt
//! `bhtune-core`'s domain enums to `clap::ValueEnum`.
//!
//! Rust's orphan rule forbids implementing a foreign trait (`clap::ValueEnum`) for a foreign
//! type (`bhtune_core::ProcessType` etc.), so each domain enum this CLI exposes as a flag
//! gets a small local wrapper here with a `From`/`Into` conversion — not a design choice, a
//! language requirement.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// `value_parser` for every `f32` CLI flag that can reach `bhtune-core` unvalidated. A
/// backend tag read is checked for finiteness in `commands::tune::read_f32`, but a CLI flag
/// value bypasses that check entirely (see `build_loop_tags`'s `TagOrValue::Value` path) --
/// without this, `--relay-amp nan` or `--sim-gain inf` would flow straight into the tuning
/// math. See AGENTS.md's "Live-plant safety hardening" section.
fn finite_f32(s: &str) -> Result<f32, String> {
    let value: f32 = s
        .parse()
        .map_err(|_| format!("'{s}' is not a valid number"))?;
    if !value.is_finite() {
        return Err(format!(
            "'{s}' must be a finite number (not NaN or infinite)"
        ));
    }
    Ok(value)
}

/// `value_parser` for a `u32` CLI flag where `0` parses fine but is nonsensical for the
/// flag's unit. `--cycles-count 0` is the motivating case: it used to reach
/// `bhtune-core::measure_oscillation`'s internal `assert!` and panic mid-run, after the loop
/// had already been switched to manual and stroked.
fn positive_u32(s: &str) -> Result<u32, String> {
    let value: u32 = s
        .parse()
        .map_err(|_| format!("'{s}' is not a valid non-negative integer"))?;
    if value == 0 {
        return Err("must be at least 1".to_string());
    }
    Ok(value)
}

/// `value_parser` for a `u64` CLI flag where `0` parses fine but is nonsensical for the
/// flag's unit -- `--poll-interval-ms 0` was previously silently clamped to `1` rather than
/// rejected, and `--timeout-secs 0` "succeeded" by aborting the run almost instantly.
fn positive_u64(s: &str) -> Result<u64, String> {
    let value: u64 = s
        .parse()
        .map_err(|_| format!("'{s}' is not a valid non-negative integer"))?;
    if value == 0 {
        return Err("must be at least 1".to_string());
    }
    Ok(value)
}

/// The `bhtune` CLI: a scriptable, no-GUI way to run an MRFT tune and inspect its history.
#[derive(Parser, Debug)]
#[command(name = "bhtune", version, about = "Headless MRFT auto-tuner")]
pub struct Cli {
    /// Path to a TOML config file (default: platform-specific, see `crate::config`).
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Path to the SQLite database file (default: a platform-standard data directory, see
    /// `crate::config::default_db_path_from`). CLI > `BHTUNE_DB` env var > `db` in the
    /// config file > platform default -- see `crate::config::resolve_db_path`.
    #[arg(long, global = true, env = "BHTUNE_DB", value_name = "PATH")]
    pub db: Option<PathBuf>,

    /// Log level / directive spec, e.g. "info" or "bhtune_cli=debug,sqlx=warn" (default:
    /// info). Diagnostic detail only -- never printed to stdout, so it can never interleave
    /// with `--output json`'s single-object contract; see `crate::logging`.
    #[arg(long, global = true, env = "RUST_LOG")]
    pub log_level: Option<String>,

    /// Directory to write log files to (default: a platform-standard data directory, see
    /// `crate::config::default_log_dir_from`).
    #[arg(long, global = true, value_name = "PATH")]
    pub log_dir: Option<PathBuf>,

    /// Log file format: "pretty" or "json" (default: pretty).
    #[arg(long, global = true)]
    pub log_format: Option<String>,

    /// Log file rotation: "hourly", "daily", or "never" (default: daily).
    #[arg(long, global = true)]
    pub log_rotation: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run an MRFT tune against a real OPC DA loop or the in-process simulator.
    Tune(TuneArgs),
    /// Run a zero-configuration demo MRFT tune against the built-in FOPDT simulator.
    Simulate(SimulateArgs),
    /// Inspect and manage DCS/PLC templates.
    Template {
        #[command(subcommand)]
        command: TemplateCommand,
    },
    /// Inspect past tune runs.
    History {
        #[command(subcommand)]
        command: HistoryCommand,
    },
    /// Export one run's recorded samples as CSV or JSON.
    Export(ExportArgs),
    /// Low-level OPC DA passthrough (diagnostics) via the opcda-bridge gateway, bypassing
    /// the tuning engine entirely.
    Opc {
        #[command(subcommand)]
        command: OpcCommand,
    },
}

impl Command {
    /// The `--output` format this command asked for, or [`crate::output::OutputFormat::Table`]
    /// for commands that don't have the concept yet (`Template`/`Export`/`Opc` -- `Export`
    /// already has its own unrelated `--output <path>` flag naming the destination file).
    /// Read before the command is dispatched (and potentially moved), so a config/database
    /// error occurring before dispatch can still be reported in the format the caller
    /// actually asked for -- see `lib.rs::run_with_cli`.
    pub(crate) fn output_format(&self) -> crate::output::OutputFormat {
        match self {
            Command::Tune(args) => args.output,
            Command::Simulate(args) => args.output,
            Command::History { command } => command.output_format(),
            Command::Template { .. } | Command::Export(_) | Command::Opc { .. } => {
                crate::output::OutputFormat::Table
            }
        }
    }
}

/// A [`bhtune_core::ProcessType`] value, as a CLI flag.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessTypeArg {
    Flow,
    PressureLine,
    PressureVessel,
    Level,
    TemperatureMixing,
    TemperatureHeatExchange,
}

impl From<ProcessTypeArg> for bhtune_core::ProcessType {
    fn from(value: ProcessTypeArg) -> Self {
        match value {
            ProcessTypeArg::Flow => bhtune_core::ProcessType::Flow,
            ProcessTypeArg::PressureLine => bhtune_core::ProcessType::PressureLine,
            ProcessTypeArg::PressureVessel => bhtune_core::ProcessType::PressureVessel,
            ProcessTypeArg::Level => bhtune_core::ProcessType::Level,
            ProcessTypeArg::TemperatureMixing => bhtune_core::ProcessType::TemperatureMixing,
            ProcessTypeArg::TemperatureHeatExchange => {
                bhtune_core::ProcessType::TemperatureHeatExchange
            }
        }
    }
}

/// A [`bhtune_core::ControllerType`] value, as a CLI flag.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerTypeArg {
    P,
    Pi,
    Pid,
}

impl From<ControllerTypeArg> for bhtune_core::ControllerType {
    fn from(value: ControllerTypeArg) -> Self {
        match value {
            ControllerTypeArg::P => bhtune_core::ControllerType::P,
            ControllerTypeArg::Pi => bhtune_core::ControllerType::Pi,
            ControllerTypeArg::Pid => bhtune_core::ControllerType::Pid,
        }
    }
}

/// A [`bhtune_core::ControllerDirection`] value, as a CLI flag.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectionArg {
    Direct,
    Reverse,
}

impl From<DirectionArg> for bhtune_core::ControllerDirection {
    fn from(value: DirectionArg) -> Self {
        match value {
            DirectionArg::Direct => bhtune_core::ControllerDirection::Direct,
            DirectionArg::Reverse => bhtune_core::ControllerDirection::Reverse,
        }
    }
}

/// Which [`bhtune_backend::Backend`] implementation a `tune` run should use.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKindArg {
    /// A real OPC DA server, reached through an opcda-bridge gateway.
    Opcda,
    /// The in-process FOPDT simulator — no external dependency at all.
    Simulator,
}

/// A [`bhtune_core::ResponseLevel`] value, as a CLI flag (`--write-pid
/// <aggressive|moderate|sluggish>`).
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseLevelArg {
    Aggressive,
    Moderate,
    Sluggish,
}

impl From<ResponseLevelArg> for bhtune_core::ResponseLevel {
    fn from(value: ResponseLevelArg) -> Self {
        match value {
            ResponseLevelArg::Aggressive => bhtune_core::ResponseLevel::Aggressive,
            ResponseLevelArg::Moderate => bhtune_core::ResponseLevel::Moderate,
            ResponseLevelArg::Sluggish => bhtune_core::ResponseLevel::Sluggish,
        }
    }
}

/// Flags shared by `tune` and (a defaulted subset of) `simulate`.
#[derive(Parser, Debug, Clone)]
pub struct TuneArgs {
    /// PV tag prefix; the rest of the tag set is derived from it using `--template`'s
    /// suffix convention. Ignored for `--backend simulator`, which uses two fixed internal
    /// tag names instead.
    #[arg(short = 't', long)]
    pub tagname: String,

    /// DCS/PLC template name (see `bhtune template list`).
    #[arg(long)]
    pub template: String,

    #[arg(long, value_enum)]
    pub process_type: ProcessTypeArg,

    #[arg(long, value_enum)]
    pub controller_type: ControllerTypeArg,

    /// Relay amplitude, as a percentage of the MV range.
    #[arg(long, value_parser = finite_f32)]
    pub relay_amp: f32,

    /// Relay cycles to skip before counting begins (default: looked up per `--process-type`).
    #[arg(long)]
    pub cycles_skip: Option<u32>,

    /// Relay cycles to count once the skip period ends (default: looked up per
    /// `--process-type`).
    #[arg(long, value_parser = positive_u32)]
    pub cycles_count: Option<u32>,

    /// Seconds a switch must persist before it's accepted (default: looked up per
    /// `--process-type`).
    #[arg(long)]
    pub noise_protection_secs: Option<u32>,

    /// Pre/post-test recording padding, in seconds (legacy: `--mrftDelayTime`).
    #[arg(long, default_value_t = 0)]
    pub mrft_delay: u32,

    /// Which backend drives this tune.
    #[arg(long, value_enum)]
    pub backend: BackendKindArg,

    /// opcda-bridge gateway address. bhtune connects to the bridge gateway rather than a
    /// DCOM host directly — see AGENTS.md's OPC DA integration notes. Only meaningful with
    /// `--backend opcda` (default: `crate::config::DEFAULT_BRIDGE_HOST`, overridable via the
    /// `BHTUNE_BRIDGE_HOST` env var or the config file's `bridge_host` key).
    #[arg(long, env = "BHTUNE_BRIDGE_HOST")]
    pub bridge_host: Option<String>,

    /// OPC DA server ProgID (legacy: `-s`/`--opcServerID`). Required with `--backend opcda`.
    #[arg(long)]
    pub server: Option<String>,

    /// Simulator process gain (`--backend simulator` only).
    #[arg(long, default_value_t = 1.0, value_parser = finite_f32)]
    pub sim_gain: f32,
    /// Simulator process time constant, in seconds (`--backend simulator` only).
    #[arg(long, default_value_t = 2.0, value_parser = finite_f32)]
    pub sim_tau: f32,
    /// Simulator dead time, in seconds (`--backend simulator` only).
    #[arg(long, default_value_t = 5.0, value_parser = finite_f32)]
    pub sim_dead_time: f32,
    /// Simulator measurement noise amplitude (`--backend simulator` only).
    #[arg(long, default_value_t = 0.0, value_parser = finite_f32)]
    pub sim_noise: f32,
    /// Simulator RNG seed, for reproducible noise (`--backend simulator` only).
    #[arg(long, default_value_t = 0)]
    pub sim_seed: u64,
    /// Simulator initial PV (`--backend simulator` only).
    #[arg(long, default_value_t = 50.0, value_parser = finite_f32)]
    pub sim_initial_pv: f32,
    /// Simulator initial MV (`--backend simulator` only).
    #[arg(long, default_value_t = 50.0, value_parser = finite_f32)]
    pub sim_initial_mv: f32,

    /// Fixed PV range high, overriding a live tag read (legacy: the PV range "toggle
    /// tag/value" button). Required (defaults to 100.0) for `--backend simulator`, which has
    /// no range tags at all.
    #[arg(long, value_parser = finite_f32)]
    pub pv_range_high: Option<f32>,
    /// Fixed PV range low, overriding a live tag read.
    #[arg(long, value_parser = finite_f32)]
    pub pv_range_low: Option<f32>,
    /// Fixed MV range high, overriding a live tag read.
    #[arg(long, value_parser = finite_f32)]
    pub mv_range_high: Option<f32>,
    /// Fixed MV range low, overriding a live tag read.
    #[arg(long, value_parser = finite_f32)]
    pub mv_range_low: Option<f32>,
    /// Fixed controller direction, overriding a live tag read.
    #[arg(long, value_enum)]
    pub direction: Option<DirectionArg>,

    /// How often to poll the backend, in milliseconds (legacy: the 800 ms WinForms timer).
    #[arg(long, default_value_t = 800, value_parser = positive_u64)]
    pub poll_interval_ms: u64,

    /// Hard wall-clock cap on this run's total duration (including any `--mrft-delay`
    /// padding), in seconds. If the engine hasn't reported completion by the deadline, the
    /// run is aborted and the loop is automatically restored, exactly like Ctrl+C -- but
    /// with no one present to press it. Always enforced; there is no way to disable it,
    /// since an unattended run must never be able to perturb a live process indefinitely.
    /// Size this to comfortably exceed your slowest loop's expected test duration --
    /// temperature loops in particular can need much longer than the default.
    #[arg(long, default_value_t = 3600, value_parser = positive_u64)]
    pub timeout_secs: u64,

    /// A friendly name for this run, recorded as `loop_name` (default: the PV tag name).
    #[arg(long)]
    pub name: Option<String>,

    /// Confirm an unattended PID write-back. Required alongside `--write-pid` -- the command
    /// refuses to start otherwise -- since writing to a live loop with no human present must
    /// be an explicit, deliberate choice. Has no effect without `--write-pid`.
    #[arg(long)]
    pub yes: bool,

    /// Non-interactively write this response level's calculated PID parameters back to the
    /// DCS instead of prompting on stdin -- the flag that makes a scheduled/scripted tune
    /// able to actually update a loop with no one watching. Requires `--yes`.
    #[arg(long, value_enum)]
    pub write_pid: Option<ResponseLevelArg>,

    /// Accept `Quality::Uncertain` OPC readings instead of hard-failing on them. Off by
    /// default: a stale/held value is indistinguishable from a live one to the MRFT engine,
    /// so tolerating it can silently corrupt the switch-period measurement the whole test
    /// depends on. Only for sites whose gateway reports `Uncertain` as a matter of course --
    /// `Quality::Bad` is never accepted, with or without this flag. Logged loudly when used
    /// and recorded on the run (`tune_runs.allow_uncertain_quality`), so history shows a run
    /// executed under relaxed rules.
    #[arg(long)]
    pub allow_uncertain_quality: bool,

    /// How to print this run's final outcome line.
    #[arg(long, value_enum, default_value = "table")]
    pub output: crate::output::OutputFormat,
}

/// `bhtune simulate`: every field defaulted for a true zero-configuration demo run.
#[derive(Parser, Debug, Clone)]
pub struct SimulateArgs {
    #[arg(short = 't', long, default_value = "Sim.Loop1.PV")]
    pub tagname: String,

    #[arg(long, default_value = "Yokogawa CentumVP")]
    pub template: String,

    #[arg(long, value_enum, default_value = "flow")]
    pub process_type: ProcessTypeArg,

    #[arg(long, value_enum, default_value = "pi")]
    pub controller_type: ControllerTypeArg,

    #[arg(long, default_value_t = 10.0, value_parser = finite_f32)]
    pub relay_amp: f32,

    #[arg(long)]
    pub cycles_skip: Option<u32>,
    #[arg(long, value_parser = positive_u32)]
    pub cycles_count: Option<u32>,
    #[arg(long)]
    pub noise_protection_secs: Option<u32>,
    #[arg(long, default_value_t = 0)]
    pub mrft_delay: u32,

    #[arg(long, default_value_t = 1.0, value_parser = finite_f32)]
    pub sim_gain: f32,
    #[arg(long, default_value_t = 2.0, value_parser = finite_f32)]
    pub sim_tau: f32,
    #[arg(long, default_value_t = 5.0, value_parser = finite_f32)]
    pub sim_dead_time: f32,
    #[arg(long, default_value_t = 0.0, value_parser = finite_f32)]
    pub sim_noise: f32,
    #[arg(long, default_value_t = 0)]
    pub sim_seed: u64,
    #[arg(long, default_value_t = 50.0, value_parser = finite_f32)]
    pub sim_initial_pv: f32,
    #[arg(long, default_value_t = 50.0, value_parser = finite_f32)]
    pub sim_initial_mv: f32,

    #[arg(long, default_value_t = 800, value_parser = positive_u64)]
    pub poll_interval_ms: u64,

    /// See `TuneArgs::timeout_secs`.
    #[arg(long, default_value_t = 3600, value_parser = positive_u64)]
    pub timeout_secs: u64,

    #[arg(long)]
    pub name: Option<String>,

    /// See `TuneArgs::yes`.
    #[arg(long)]
    pub yes: bool,

    /// See `TuneArgs::write_pid`. Note the built-in FOPDT simulator has no PID constant
    /// tags at all (see `build_loop_tags`), so write-back is always skipped for `simulate`
    /// regardless of this flag -- it's accepted here purely so `simulate`'s flag surface
    /// stays a strict defaulted subset of `tune`'s, matching every other field.
    #[arg(long, value_enum)]
    pub write_pid: Option<ResponseLevelArg>,

    /// See `TuneArgs::allow_uncertain_quality`.
    #[arg(long)]
    pub allow_uncertain_quality: bool,

    /// See `TuneArgs::output`.
    #[arg(long, value_enum, default_value = "table")]
    pub output: crate::output::OutputFormat,
}

impl SimulateArgs {
    /// Expands the defaulted `simulate` flags into a full [`TuneArgs`] with
    /// `--backend simulator` implied, so `simulate` and `tune` share one execution path.
    pub fn into_tune_args(self) -> TuneArgs {
        TuneArgs {
            tagname: self.tagname,
            template: self.template,
            process_type: self.process_type,
            controller_type: self.controller_type,
            relay_amp: self.relay_amp,
            cycles_skip: self.cycles_skip,
            cycles_count: self.cycles_count,
            noise_protection_secs: self.noise_protection_secs,
            mrft_delay: self.mrft_delay,
            backend: BackendKindArg::Simulator,
            bridge_host: None,
            server: None,
            sim_gain: self.sim_gain,
            sim_tau: self.sim_tau,
            sim_dead_time: self.sim_dead_time,
            sim_noise: self.sim_noise,
            sim_seed: self.sim_seed,
            sim_initial_pv: self.sim_initial_pv,
            sim_initial_mv: self.sim_initial_mv,
            // The simulator has no range/direction tags at all (see `backend-simulator`'s
            // two-tag-only contract), so these must always be fixed values, defaulted to a
            // plain 0-100% span and the direction already proven to produce a completing
            // relay test in `bhtune-backend`'s own end-to-end test.
            pv_range_high: Some(100.0),
            pv_range_low: Some(0.0),
            mv_range_high: Some(100.0),
            mv_range_low: Some(0.0),
            direction: Some(DirectionArg::Reverse),
            poll_interval_ms: self.poll_interval_ms,
            timeout_secs: self.timeout_secs,
            name: self.name,
            yes: self.yes,
            write_pid: self.write_pid,
            allow_uncertain_quality: self.allow_uncertain_quality,
            output: self.output,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum TemplateCommand {
    /// List every template (built-in and user-imported).
    List,
    /// Show one template's full detail as JSON.
    Show { name: String },
    /// Import a template from a JSON file (see `template export`'s output shape).
    Import { path: PathBuf },
    /// Export a template's JSON to a file, e.g. as a starting point for a site-specific copy.
    Export { name: String, path: PathBuf },
}

#[derive(Subcommand, Debug)]
pub enum HistoryCommand {
    /// List past runs, newest first.
    List {
        #[arg(long)]
        outcome: Option<OutcomeArg>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
        #[arg(long, default_value_t = 0)]
        offset: i64,
        /// How to print the run list.
        #[arg(long, value_enum, default_value = "table")]
        output: crate::output::OutputFormat,
    },
    /// Show one run's full detail: config, initial readings, calculated results, and any
    /// PID write-back audit rows.
    Show {
        run_id: i64,
        /// How to print the run detail.
        #[arg(long, value_enum, default_value = "table")]
        output: crate::output::OutputFormat,
    },
}

impl HistoryCommand {
    pub(crate) fn output_format(&self) -> crate::output::OutputFormat {
        match self {
            HistoryCommand::List { output, .. } => *output,
            HistoryCommand::Show { output, .. } => *output,
        }
    }
}

/// A [`bhtune_db::models::TuneOutcome`] value, as a CLI flag.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeArg {
    Running,
    Completed,
    Failed,
    Aborted,
}

impl From<OutcomeArg> for bhtune_db::models::TuneOutcome {
    fn from(value: OutcomeArg) -> Self {
        match value {
            OutcomeArg::Running => bhtune_db::models::TuneOutcome::Running,
            OutcomeArg::Completed => bhtune_db::models::TuneOutcome::Completed,
            OutcomeArg::Failed => bhtune_db::models::TuneOutcome::Failed,
            OutcomeArg::Aborted => bhtune_db::models::TuneOutcome::Aborted,
        }
    }
}

#[derive(Parser, Debug)]
pub struct ExportArgs {
    pub run_id: i64,
    #[arg(long, value_enum, default_value = "csv")]
    pub format: ExportFormat,
    /// Output file path (default: stdout).
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Json,
}

#[derive(Subcommand, Debug)]
pub enum OpcCommand {
    /// Read one or more tags.
    Read {
        /// (default: `crate::config::DEFAULT_BRIDGE_HOST`, overridable via `BHTUNE_BRIDGE_HOST`
        /// or the config file's `bridge_host` key.)
        #[arg(long, env = "BHTUNE_BRIDGE_HOST")]
        bridge_host: Option<String>,
        /// (default: the config file's `server` key; errors if neither is set.)
        #[arg(long)]
        server: Option<String>,
        tags: Vec<String>,
    },
    /// Write a value to one tag.
    Write {
        #[arg(long, env = "BHTUNE_BRIDGE_HOST")]
        bridge_host: Option<String>,
        #[arg(long)]
        server: Option<String>,
        tag: String,
        value: String,
    },
    /// Browse tags under a path (empty for the top level).
    Browse {
        #[arg(long, env = "BHTUNE_BRIDGE_HOST")]
        bridge_host: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(default_value = "")]
        path: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Downcasts `$value` to `$pattern`'s binding, panicking with `expected $label` otherwise.
    /// Every test below that parses a `Cli`/subcommand enum needs exactly this "or fail the
    /// test clearly" step; sharing one macro (rather than each call site's own `let-else {
    /// panic!(...) }`) means there is exactly one such panic branch in this file instead of
    /// four near-identical, individually-uncovered ones. `expect_variant_panics_on_a_mismatch`
    /// below is a dedicated test that deliberately trips it, so this one shared branch is
    /// itself covered rather than becoming a permanent, accepted gap.
    macro_rules! expect_variant {
        ($value:expr, $pattern:pat => $binding:expr, $label:literal) => {
            match $value {
                $pattern => $binding,
                _ => panic!("expected {}", $label),
            }
        };
    }

    #[test]
    fn expect_variant_panics_on_a_mismatch() {
        let command = Cli::parse_from(["bhtune", "simulate"]).command;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || expect_variant!(command, Command::Tune(a) => a, "Tune"),
        ));
        let panic_message = *result.unwrap_err().downcast::<&str>().unwrap();
        assert_eq!(panic_message, "expected Tune");
    }

    #[test]
    fn process_type_arg_converts_to_every_core_variant() {
        assert_eq!(
            bhtune_core::ProcessType::from(ProcessTypeArg::Flow),
            bhtune_core::ProcessType::Flow
        );
        assert_eq!(
            bhtune_core::ProcessType::from(ProcessTypeArg::PressureLine),
            bhtune_core::ProcessType::PressureLine
        );
        assert_eq!(
            bhtune_core::ProcessType::from(ProcessTypeArg::PressureVessel),
            bhtune_core::ProcessType::PressureVessel
        );
        assert_eq!(
            bhtune_core::ProcessType::from(ProcessTypeArg::Level),
            bhtune_core::ProcessType::Level
        );
        assert_eq!(
            bhtune_core::ProcessType::from(ProcessTypeArg::TemperatureMixing),
            bhtune_core::ProcessType::TemperatureMixing
        );
        assert_eq!(
            bhtune_core::ProcessType::from(ProcessTypeArg::TemperatureHeatExchange),
            bhtune_core::ProcessType::TemperatureHeatExchange
        );
    }

    #[test]
    fn controller_type_arg_converts_to_every_core_variant() {
        assert_eq!(
            bhtune_core::ControllerType::from(ControllerTypeArg::P),
            bhtune_core::ControllerType::P
        );
        assert_eq!(
            bhtune_core::ControllerType::from(ControllerTypeArg::Pi),
            bhtune_core::ControllerType::Pi
        );
        assert_eq!(
            bhtune_core::ControllerType::from(ControllerTypeArg::Pid),
            bhtune_core::ControllerType::Pid
        );
    }

    #[test]
    fn direction_arg_converts_to_every_core_variant() {
        assert_eq!(
            bhtune_core::ControllerDirection::from(DirectionArg::Direct),
            bhtune_core::ControllerDirection::Direct
        );
        assert_eq!(
            bhtune_core::ControllerDirection::from(DirectionArg::Reverse),
            bhtune_core::ControllerDirection::Reverse
        );
    }

    #[test]
    fn outcome_arg_converts_to_every_db_variant() {
        assert_eq!(
            bhtune_db::models::TuneOutcome::from(OutcomeArg::Running),
            bhtune_db::models::TuneOutcome::Running
        );
        assert_eq!(
            bhtune_db::models::TuneOutcome::from(OutcomeArg::Completed),
            bhtune_db::models::TuneOutcome::Completed
        );
        assert_eq!(
            bhtune_db::models::TuneOutcome::from(OutcomeArg::Failed),
            bhtune_db::models::TuneOutcome::Failed
        );
        assert_eq!(
            bhtune_db::models::TuneOutcome::from(OutcomeArg::Aborted),
            bhtune_db::models::TuneOutcome::Aborted
        );
    }

    #[test]
    fn simulate_args_expand_into_tune_args_with_simulator_backend() {
        let cli = Cli::parse_from(["bhtune", "simulate"]);
        let simulate = expect_variant!(cli.command, Command::Simulate(s) => s, "Simulate");
        let tune = simulate.into_tune_args();
        assert!(matches!(tune.backend, BackendKindArg::Simulator));
        assert_eq!(tune.tagname, "Sim.Loop1.PV");
        assert_eq!(tune.pv_range_low, Some(0.0));
        assert_eq!(tune.pv_range_high, Some(100.0));
        assert_eq!(tune.mv_range_low, Some(0.0));
        assert_eq!(tune.mv_range_high, Some(100.0));
        assert!(matches!(tune.direction, Some(DirectionArg::Reverse)));
        assert!(!tune.yes);
        assert!(tune.write_pid.is_none());
        assert_eq!(tune.output, crate::output::OutputFormat::Table);
    }

    #[test]
    fn simulate_args_expand_into_tune_args_carries_yes_write_pid_and_output_through() {
        let cli = Cli::parse_from([
            "bhtune",
            "simulate",
            "--yes",
            "--write-pid",
            "sluggish",
            "--output",
            "json",
        ]);
        let simulate = expect_variant!(cli.command, Command::Simulate(s) => s, "Simulate");
        let tune = simulate.into_tune_args();
        assert!(tune.yes);
        assert!(matches!(tune.write_pid, Some(ResponseLevelArg::Sluggish)));
        assert_eq!(tune.output, crate::output::OutputFormat::Json);
    }

    #[test]
    fn response_level_arg_converts_to_every_core_variant() {
        assert_eq!(
            bhtune_core::ResponseLevel::from(ResponseLevelArg::Aggressive),
            bhtune_core::ResponseLevel::Aggressive
        );
        assert_eq!(
            bhtune_core::ResponseLevel::from(ResponseLevelArg::Moderate),
            bhtune_core::ResponseLevel::Moderate
        );
        assert_eq!(
            bhtune_core::ResponseLevel::from(ResponseLevelArg::Sluggish),
            bhtune_core::ResponseLevel::Sluggish
        );
    }

    #[test]
    fn cli_parses_a_full_tune_command() {
        let cli = Cli::parse_from([
            "bhtune",
            "tune",
            "-t",
            "Unit1.LIC101.PV",
            "--template",
            "Yokogawa CentumVP",
            "--process-type",
            "flow",
            "--controller-type",
            "pi",
            "--relay-amp",
            "5.0",
            "--backend",
            "simulator",
        ]);
        let args = expect_variant!(cli.command, Command::Tune(a) => a, "Tune");
        assert_eq!(args.tagname, "Unit1.LIC101.PV");
        assert!(matches!(args.process_type, ProcessTypeArg::Flow));
        assert!(matches!(args.controller_type, ControllerTypeArg::Pi));
        assert!(matches!(args.backend, BackendKindArg::Simulator));
        assert_eq!(args.poll_interval_ms, 800);
        assert!(!args.yes);
        assert!(args.write_pid.is_none());
        assert_eq!(args.output, crate::output::OutputFormat::Table);
    }

    #[test]
    fn cli_parses_tune_yes_and_write_pid_flags() {
        let cli = Cli::parse_from([
            "bhtune",
            "tune",
            "-t",
            "Unit1.LIC101.PV",
            "--template",
            "Yokogawa CentumVP",
            "--process-type",
            "flow",
            "--controller-type",
            "pi",
            "--relay-amp",
            "5.0",
            "--backend",
            "simulator",
            "--yes",
            "--write-pid",
            "moderate",
            "--output",
            "json",
        ]);
        let args = expect_variant!(cli.command, Command::Tune(a) => a, "Tune");
        assert!(args.yes);
        assert!(matches!(args.write_pid, Some(ResponseLevelArg::Moderate)));
        assert_eq!(args.output, crate::output::OutputFormat::Json);
    }

    #[test]
    fn cli_parses_history_list_with_filters() {
        let cli = Cli::parse_from([
            "bhtune",
            "history",
            "list",
            "--outcome",
            "completed",
            "--limit",
            "10",
        ]);
        let command =
            expect_variant!(cli.command, Command::History { command } => command, "History");
        let (outcome, limit, offset, output) = expect_variant!(
            command,
            HistoryCommand::List { outcome, limit, offset, output } => (outcome, limit, offset, output),
            "List"
        );
        assert!(matches!(outcome, Some(OutcomeArg::Completed)));
        assert_eq!(limit, 10);
        assert_eq!(offset, 0);
        assert_eq!(output, crate::output::OutputFormat::Table);
    }

    #[test]
    fn cli_parses_history_list_and_show_with_output_json() {
        let cli = Cli::parse_from(["bhtune", "history", "list", "--output", "json"]);
        let command =
            expect_variant!(cli.command, Command::History { command } => command, "History");
        assert_eq!(command.output_format(), crate::output::OutputFormat::Json);

        let cli = Cli::parse_from(["bhtune", "history", "show", "42", "--output", "json"]);
        let command =
            expect_variant!(cli.command, Command::History { command } => command, "History");
        let (run_id, output) = expect_variant!(
            command,
            HistoryCommand::Show { run_id, output } => (run_id, output),
            "Show"
        );
        assert_eq!(run_id, 42);
        assert_eq!(output, crate::output::OutputFormat::Json);
    }

    #[test]
    fn command_output_format_defaults_to_table_for_commands_without_the_concept() {
        let cli = Cli::parse_from(["bhtune", "template", "list"]);
        assert_eq!(
            cli.command.output_format(),
            crate::output::OutputFormat::Table
        );

        let cli = Cli::parse_from(["bhtune", "opc", "read", "Unit1.LIC101.PV"]);
        assert_eq!(
            cli.command.output_format(),
            crate::output::OutputFormat::Table
        );

        let cli = Cli::parse_from(["bhtune", "export", "1"]);
        assert_eq!(
            cli.command.output_format(),
            crate::output::OutputFormat::Table
        );
    }

    #[test]
    fn cli_rejects_missing_required_tune_flags() {
        let result = Cli::try_parse_from(["bhtune", "tune"]);
        assert!(result.is_err());
    }

    #[test]
    fn cli_db_and_config_default_to_none() {
        let cli = Cli::parse_from(["bhtune", "simulate"]);
        assert_eq!(cli.db, None);
        assert_eq!(cli.config, None);
    }

    #[test]
    fn cli_parses_explicit_db_and_config_flags() {
        let cli = Cli::parse_from([
            "bhtune",
            "--db",
            "/data/bhtune.db",
            "--config",
            "/etc/bhtune.toml",
            "simulate",
        ]);
        assert_eq!(cli.db, Some(PathBuf::from("/data/bhtune.db")));
        assert_eq!(cli.config, Some(PathBuf::from("/etc/bhtune.toml")));
    }

    #[test]
    fn cli_log_flags_default_to_none() {
        let cli = Cli::parse_from(["bhtune", "simulate"]);
        assert_eq!(cli.log_level, None);
        assert_eq!(cli.log_dir, None);
        assert_eq!(cli.log_format, None);
        assert_eq!(cli.log_rotation, None);
    }

    #[test]
    fn cli_parses_explicit_log_flags() {
        let cli = Cli::parse_from([
            "bhtune",
            "--log-level",
            "debug",
            "--log-dir",
            "/var/log/bhtune",
            "--log-format",
            "json",
            "--log-rotation",
            "hourly",
            "simulate",
        ]);
        assert_eq!(cli.log_level, Some("debug".to_string()));
        assert_eq!(cli.log_dir, Some(PathBuf::from("/var/log/bhtune")));
        assert_eq!(cli.log_format, Some("json".to_string()));
        assert_eq!(cli.log_rotation, Some("hourly".to_string()));
    }

    #[test]
    fn tune_args_bridge_host_defaults_to_none() {
        let cli = Cli::parse_from([
            "bhtune",
            "tune",
            "-t",
            "Unit1.LIC101.PV",
            "--template",
            "Yokogawa CentumVP",
            "--process-type",
            "flow",
            "--controller-type",
            "pi",
            "--relay-amp",
            "5.0",
            "--backend",
            "simulator",
        ]);
        let args = expect_variant!(cli.command, Command::Tune(a) => a, "Tune");
        assert_eq!(args.bridge_host, None);
    }

    #[test]
    fn opc_read_bridge_host_and_server_default_to_none() {
        let cli = Cli::parse_from(["bhtune", "opc", "read", "Unit1.LIC101.PV"]);
        let command = expect_variant!(cli.command, Command::Opc { command } => command, "Opc");
        let (bridge_host, server) = expect_variant!(
            command,
            OpcCommand::Read { bridge_host, server, .. } => (bridge_host, server),
            "Read"
        );
        assert_eq!(bridge_host, None);
        assert_eq!(server, None);
    }
}
