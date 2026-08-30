//! Command-line argument definitions (`clap` derive) and the small wrapper enums that adapt
//! `bhtune-core`'s domain enums to `clap::ValueEnum`.
//!
//! Rust's orphan rule forbids implementing a foreign trait (`clap::ValueEnum`) for a foreign
//! type (`bhtune_core::ProcessType` etc.), so each domain enum this CLI exposes as a flag
//! gets a small local wrapper here with a `From`/`Into` conversion — not a design choice, a
//! language requirement.

use std::path::PathBuf;

use bhtune_core::TagOverrides;
use clap::{Parser, Subcommand, ValueEnum};

/// `value_parser` for every `f32` CLI flag that can reach `bhtune-core` unvalidated. A
/// driver tag read is checked for finiteness in `commands::tune::read_f32`, but a CLI flag
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

    /// Path to a user-supplied DCS/PLC template catalog, auto-loaded on every startup in
    /// addition to the built-in templates (default: platform-specific, next to the config
    /// file -- see `crate::config::templates_path_from`). A missing file at the default
    /// location is fine; a file that fails to parse or validate is a hard error. CLI >
    /// `BHTUNE_TEMPLATES` env var > `templates` in the config file > platform default --
    /// see `crate::config::load_user_templates`.
    #[arg(long, global = true, env = "BHTUNE_TEMPLATES", value_name = "PATH")]
    pub templates: Option<PathBuf>,

    /// Delete tune runs (and their samples/results/write-back audit rows) older than this
    /// many days, automatically, on every startup (default: unset -- retain forever). CLI >
    /// `BHTUNE_RETENTION_DAYS` env var > `retention_days` in the config file > (no default)
    /// -- see `crate::config::resolve_retention_days`. `bhtune history prune` applies the
    /// same policy on demand, with a `--dry-run` preview, instead of waiting for the next
    /// startup.
    #[arg(long, global = true, env = "BHTUNE_RETENTION_DAYS", value_parser = positive_u32)]
    pub retention_days: Option<u32>,

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
#[allow(clippy::large_enum_variant)]
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

/// The reverse of the `impl From<ProcessTypeArg>` above -- needed by `bhtune-server`'s
/// `POST /api/runs`, whose request body deserializes straight into `bhtune-core`'s domain
/// enums (already `Deserialize`/`ToSchema` via existing feature-gating, and meaningful
/// outside a CLI context) rather than these CLI-only `clap::ValueEnum` wrappers, then
/// converts into a [`TuneArgs`] to reuse this crate's tune orchestration unchanged.
impl From<bhtune_core::ProcessType> for ProcessTypeArg {
    fn from(value: bhtune_core::ProcessType) -> Self {
        match value {
            bhtune_core::ProcessType::Flow => ProcessTypeArg::Flow,
            bhtune_core::ProcessType::PressureLine => ProcessTypeArg::PressureLine,
            bhtune_core::ProcessType::PressureVessel => ProcessTypeArg::PressureVessel,
            bhtune_core::ProcessType::Level => ProcessTypeArg::Level,
            bhtune_core::ProcessType::TemperatureMixing => ProcessTypeArg::TemperatureMixing,
            bhtune_core::ProcessType::TemperatureHeatExchange => {
                ProcessTypeArg::TemperatureHeatExchange
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

/// The reverse of the `impl From<ControllerTypeArg>` above -- see
/// `impl From<bhtune_core::ProcessType> for ProcessTypeArg`'s doc comment for why.
impl From<bhtune_core::ControllerType> for ControllerTypeArg {
    fn from(value: bhtune_core::ControllerType) -> Self {
        match value {
            bhtune_core::ControllerType::P => ControllerTypeArg::P,
            bhtune_core::ControllerType::Pi => ControllerTypeArg::Pi,
            bhtune_core::ControllerType::Pid => ControllerTypeArg::Pid,
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

/// The reverse of the `impl From<DirectionArg>` above -- see
/// `impl From<bhtune_core::ProcessType> for ProcessTypeArg`'s doc comment for why.
impl From<bhtune_core::ControllerDirection> for DirectionArg {
    fn from(value: bhtune_core::ControllerDirection) -> Self {
        match value {
            bhtune_core::ControllerDirection::Direct => DirectionArg::Direct,
            bhtune_core::ControllerDirection::Reverse => DirectionArg::Reverse,
        }
    }
}

/// Which [`bhtune_driver::Driver`] implementation a `tune` run should use.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverKindArg {
    /// A real OPC DA server, reached through an opcda-bridge gateway.
    Opcda,
    /// The in-process FOPDT simulator — no external dependency at all.
    Simulator,
}

/// The reverse of `DriverKindArg -> TuneDriver` conversions elsewhere in this crate, for
/// `bhtune-server`'s `POST /api/runs`, whose request body accepts
/// [`bhtune_db::models::TuneDriver`] directly (already `Deserialize`/`ToSchema`, and the one
/// enum every other run-history route already uses on the wire -- see `routes/history.rs` in
/// `bhtune-server`) rather than this CLI-only wrapper.
///
/// `TryFrom`, not `From`: [`bhtune_db::models::TuneDriver::Replay`] has no [`DriverKindArg`]
/// counterpart at all yet (`driver-replay` in AGENTS.md is still unimplemented, so there is
/// no `crate::driver::build` case that could ever construct one), so a request naming it
/// must be rejected explicitly rather than silently mapped to something else.
impl TryFrom<bhtune_db::models::TuneDriver> for DriverKindArg {
    type Error = ReplayDriverUnsupported;

    fn try_from(value: bhtune_db::models::TuneDriver) -> Result<Self, Self::Error> {
        match value {
            bhtune_db::models::TuneDriver::Opcda => Ok(DriverKindArg::Opcda),
            bhtune_db::models::TuneDriver::Simulator => Ok(DriverKindArg::Simulator),
            bhtune_db::models::TuneDriver::Replay => Err(ReplayDriverUnsupported),
        }
    }
}

/// The error [`DriverKindArg::try_from`] returns for
/// [`bhtune_db::models::TuneDriver::Replay`] -- see that impl's doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayDriverUnsupported;

impl std::fmt::Display for ReplayDriverUnsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the replay driver cannot be used to start a new tune run (it exists only for \
             offline golden-trace validation, not live/simulated tuning)"
        )
    }
}

impl std::error::Error for ReplayDriverUnsupported {}

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

/// The reverse of the `impl From<ResponseLevelArg>` above -- see
/// `impl From<bhtune_core::ProcessType> for ProcessTypeArg`'s doc comment for why.
impl From<bhtune_core::ResponseLevel> for ResponseLevelArg {
    fn from(value: bhtune_core::ResponseLevel) -> Self {
        match value {
            bhtune_core::ResponseLevel::Aggressive => ResponseLevelArg::Aggressive,
            bhtune_core::ResponseLevel::Moderate => ResponseLevelArg::Moderate,
            bhtune_core::ResponseLevel::Sluggish => ResponseLevelArg::Sluggish,
        }
    }
}

/// Flags shared by `tune` and (a defaulted subset of) `simulate`.
#[derive(Parser, Debug, Clone)]
pub struct TuneArgs {
    /// PV tag prefix; the rest of the tag set is derived from it using `--template`'s
    /// suffix convention. Ignored for `--driver simulator`, which uses two fixed internal
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

    /// Which driver drives this tune.
    #[arg(long, value_enum)]
    pub driver: DriverKindArg,

    /// opcda-bridge gateway address. bhtune connects to the bridge gateway rather than a
    /// DCOM host directly — see AGENTS.md's OPC DA integration notes. Only meaningful with
    /// `--driver opcda` (default: `crate::config::DEFAULT_BRIDGE_HOST`, overridable via the
    /// `BHTUNE_BRIDGE_HOST` env var or the config file's `bridge_host` key).
    #[arg(long, env = "BHTUNE_BRIDGE_HOST")]
    pub bridge_host: Option<String>,

    /// OPC DA server ProgID (legacy: `-s`/`--opcServerID`). Required with `--driver opcda`.
    #[arg(long)]
    pub server: Option<String>,

    /// Simulator process gain (`--driver simulator` only).
    #[arg(long, default_value_t = 1.0, value_parser = finite_f32)]
    pub sim_gain: f32,
    /// Simulator process time constant, in seconds (`--driver simulator` only).
    #[arg(long, default_value_t = 2.0, value_parser = finite_f32)]
    pub sim_tau: f32,
    /// Simulator dead time, in seconds (`--driver simulator` only).
    #[arg(long, default_value_t = 5.0, value_parser = finite_f32)]
    pub sim_dead_time: f32,
    /// Simulator measurement noise amplitude (`--driver simulator` only).
    #[arg(long, default_value_t = 0.0, value_parser = finite_f32)]
    pub sim_noise: f32,
    /// Simulator RNG seed, for reproducible noise (`--driver simulator` only).
    #[arg(long, default_value_t = 0)]
    pub sim_seed: u64,
    /// Simulator initial PV (`--driver simulator` only).
    #[arg(long, default_value_t = 50.0, value_parser = finite_f32)]
    pub sim_initial_pv: f32,
    /// Simulator initial MV (`--driver simulator` only).
    #[arg(long, default_value_t = 50.0, value_parser = finite_f32)]
    pub sim_initial_mv: f32,

    /// Fixed PV range high, overriding a live tag read (legacy: the PV range "toggle
    /// tag/value" button). Required (defaults to 100.0) for `--driver simulator`, which has
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

    /// Per-tune replacements for template-derived tag names. This is populated by the HTTP
    /// API/UI; the CLI has no separate flags for the nested object.
    #[arg(skip)]
    pub tag_overrides: Option<TagOverrides>,

    /// How often to poll the driver, in milliseconds (legacy: the 800 ms WinForms timer).
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

    /// Operator notes to attach to this run. Notes can be edited or cleared from the web GUI
    /// while the run is active or after it finishes.
    #[arg(long)]
    pub notes: Option<String>,

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

    /// Cap on any single driver read/write during the run, in seconds. A stalled call
    /// (gateway down, DCOM wedged, network black-holed) is abandoned rather than awaited
    /// forever once this elapses, so Ctrl+C and `--timeout-secs` both stay effective even
    /// mid-hung-read/write -- see AGENTS.md's `safety-cancellation` section. Distinct from
    /// `--timeout-secs`, which bounds the whole run rather than one operation; size this
    /// well above a healthy round trip to your OPC DA gateway, not to the expected test
    /// duration.
    #[arg(long, default_value_t = 30, value_parser = positive_u64)]
    pub op_timeout_secs: u64,

    /// Cap on restoring the loop to its pre-test mode/MV/setpoint after the run ends (by
    /// completion, Ctrl+C, or a timeout), in seconds. Bounded independently of
    /// `--timeout-secs`, since a restore triggered *by* a timeout would otherwise inherit an
    /// already-expired budget. If this elapses (or a second Ctrl+C arrives first), the run
    /// exits `EXIT_RESTORE_INCOMPLETE` with a warning naming the loop and its last-written
    /// value, instead of hanging indefinitely.
    #[arg(long, default_value_t = 30, value_parser = positive_u64)]
    pub restore_timeout_secs: u64,

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

    /// Operator notes to attach to this run. See [`TuneArgs::notes`].
    #[arg(long)]
    pub notes: Option<String>,

    /// See `TuneArgs::yes`.
    #[arg(long)]
    pub yes: bool,

    /// See `TuneArgs::write_pid`. Note the built-in FOPDT simulator has no PID constant
    /// tags at all (see `build_loop_tags`), so write-back is always skipped for `simulate`
    /// regardless of this flag -- it's accepted here purely so `simulate`'s flag surface
    /// stays a strict defaulted subset of `tune`'s, matching every other field.
    #[arg(long, value_enum)]
    pub write_pid: Option<ResponseLevelArg>,

    /// See `TuneArgs::op_timeout_secs`.
    #[arg(long, default_value_t = 30, value_parser = positive_u64)]
    pub op_timeout_secs: u64,

    /// See `TuneArgs::restore_timeout_secs`.
    #[arg(long, default_value_t = 30, value_parser = positive_u64)]
    pub restore_timeout_secs: u64,

    /// See `TuneArgs::output`.
    #[arg(long, value_enum, default_value = "table")]
    pub output: crate::output::OutputFormat,
}

impl SimulateArgs {
    /// Expands the defaulted `simulate` flags into a full [`TuneArgs`] with
    /// `--driver simulator` implied, so `simulate` and `tune` share one execution path.
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
            driver: DriverKindArg::Simulator,
            bridge_host: None,
            server: None,
            sim_gain: self.sim_gain,
            sim_tau: self.sim_tau,
            sim_dead_time: self.sim_dead_time,
            sim_noise: self.sim_noise,
            sim_seed: self.sim_seed,
            sim_initial_pv: self.sim_initial_pv,
            sim_initial_mv: self.sim_initial_mv,
            // The simulator has no range/direction tags at all (see `driver-simulator`'s
            // two-tag-only contract), so these must always be fixed values, defaulted to a
            // plain 0-100% span and the direction already proven to produce a completing
            // relay test in `bhtune-driver`'s own end-to-end test.
            pv_range_high: Some(100.0),
            pv_range_low: Some(0.0),
            mv_range_high: Some(100.0),
            mv_range_low: Some(0.0),
            direction: Some(DirectionArg::Reverse),
            tag_overrides: None,
            poll_interval_ms: self.poll_interval_ms,
            timeout_secs: self.timeout_secs,
            notes: self.notes,
            yes: self.yes,
            write_pid: self.write_pid,
            op_timeout_secs: self.op_timeout_secs,
            restore_timeout_secs: self.restore_timeout_secs,
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
    /// Import a template from a file. Accepts either a single template as JSON (see
    /// `template export`'s default output shape) or a multi-template TOML catalog (the same
    /// `[[template]]` array-of-tables shape as the embedded/user catalog, see `template
    /// export --format toml`) -- the format is auto-detected from the file's content, not
    /// its extension. A JSON single-template import is rejected outright if a template with
    /// that name already exists; a TOML catalog import instead skips (and reports) any
    /// template whose name already exists, so re-importing an updated community catalog
    /// only adds what's new.
    Import { path: PathBuf },
    /// Export a template to a file, e.g. as a starting point for a site-specific copy or a
    /// community catalog contribution.
    Export {
        name: String,
        path: PathBuf,
        /// File format to write. `toml` emits a single-entry `[[template]]` catalog block,
        /// ready to paste into a catalog file or open as a contribution pull request.
        #[arg(long, value_enum, default_value = "json")]
        format: TemplateFileFormat,
    },
    /// Delete a template. Refuses if any saved loop still references it. A `Builtin`- or
    /// `Catalog`-origin template reappears automatically the next time bhtune starts unless
    /// it's also removed from its source (bhtune-core's embedded catalog for `Builtin`,
    /// which only a new bhtune release can change; the user catalog file for `Catalog`).
    Delete { name: String },
}

/// File format for `template export`/auto-detected on `template import`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateFileFormat {
    Json,
    Toml,
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
    /// Undo a run's PID write-back, writing its recorded pre-write P/I/D values back to the
    /// live loop. Reverts whichever `write`-kind write-back that run last recorded; refuses
    /// if the run has none, if that write-back's pre-read itself failed (nothing to revert
    /// to), or if the run did not use the `opcda` driver (nothing live to revert against).
    Revert {
        run_id: i64,
        /// Cross-checked against the run's own recorded bridge host -- never used to resolve
        /// a default, and deliberately has no `BHTUNE_BRIDGE_HOST`/config fallback the way
        /// every other command's `--bridge-host` does, so an unrelated ambient env var can
        /// never silently affect which gateway a revert targets. Omit this to use the
        /// recorded value; a value that contradicts it is refused rather than preferred, so
        /// a revert can never target a different gateway than the run it is undoing actually
        /// used (`db-run-request-snapshot`).
        #[arg(long)]
        bridge_host: Option<String>,
        /// Cross-checked against the run's own recorded OPC server -- never used to resolve
        /// a default. Omit this to use the recorded value; a value that contradicts it is
        /// refused rather than preferred, so a revert can never target a different server
        /// than the run it is undoing actually used (`db-run-request-snapshot`).
        #[arg(long)]
        server: Option<String>,
        /// Confirm writing to a live loop. Required -- there is no interactive prompt for
        /// reverting, since there is no calculated result to choose between as there is for
        /// `tune`'s own write-back step.
        #[arg(long)]
        yes: bool,
        /// How to print the revert outcome.
        #[arg(long, value_enum, default_value = "table")]
        output: crate::output::OutputFormat,
    },
    /// Delete runs older than the configured retention policy (`history-retention`), without
    /// waiting for the next automatic startup sweep.
    Prune {
        /// Delete runs older than this many days, overriding the configured `retention_days`
        /// policy for this invocation only. Required if no retention policy is configured at
        /// all (`--retention-days` / `BHTUNE_RETENTION_DAYS` / the config file's
        /// `retention_days` key) -- there is no default "prune everything older than X" to
        /// fall back to.
        #[arg(long, value_parser = positive_u32)]
        older_than_days: Option<u32>,
        /// Report how many runs would be deleted, and as of what cutoff, without deleting
        /// anything.
        #[arg(long)]
        dry_run: bool,
        /// How to print the prune outcome.
        #[arg(long, value_enum, default_value = "table")]
        output: crate::output::OutputFormat,
    },
}

impl HistoryCommand {
    pub(crate) fn output_format(&self) -> crate::output::OutputFormat {
        match self {
            HistoryCommand::List { output, .. } => *output,
            HistoryCommand::Show { output, .. } => *output,
            HistoryCommand::Revert { output, .. } => *output,
            HistoryCommand::Prune { output, .. } => *output,
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
    /// List the OPC DA servers registered on the bridge gateway's host.
    Servers {
        /// (default: `crate::config::DEFAULT_BRIDGE_HOST`, overridable via `BHTUNE_BRIDGE_HOST`
        /// or the config file's `bridge_host` key.)
        #[arg(long, env = "BHTUNE_BRIDGE_HOST")]
        bridge_host: Option<String>,
    },
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
    fn history_command_output_format_covers_every_variant() {
        assert_eq!(
            HistoryCommand::List {
                outcome: None,
                limit: 50,
                offset: 0,
                output: crate::output::OutputFormat::Json,
            }
            .output_format(),
            crate::output::OutputFormat::Json
        );
        assert_eq!(
            HistoryCommand::Show {
                run_id: 1,
                output: crate::output::OutputFormat::Json,
            }
            .output_format(),
            crate::output::OutputFormat::Json
        );
        assert_eq!(
            HistoryCommand::Revert {
                run_id: 1,
                bridge_host: None,
                server: None,
                yes: false,
                output: crate::output::OutputFormat::Json,
            }
            .output_format(),
            crate::output::OutputFormat::Json
        );
        assert_eq!(
            HistoryCommand::Prune {
                older_than_days: None,
                dry_run: false,
                output: crate::output::OutputFormat::Json,
            }
            .output_format(),
            crate::output::OutputFormat::Json
        );
    }

    #[test]
    fn command_output_format_forwards_simulate_and_history_formats() {
        let simulate = Cli::parse_from(["bhtune", "simulate", "--output", "json"]).command;
        assert_eq!(simulate.output_format(), crate::output::OutputFormat::Json);

        let history = Cli::parse_from(["bhtune", "history", "list", "--output", "json"]).command;
        assert_eq!(history.output_format(), crate::output::OutputFormat::Json);
    }

    #[test]
    fn finite_f32_accepts_finite_values_and_rejects_invalid_values() {
        assert_eq!(finite_f32("2.5").unwrap(), 2.5);
        assert!(finite_f32("not-a-number").is_err());
        assert!(finite_f32("nan").is_err());
        assert!(finite_f32("inf").is_err());
    }

    #[test]
    fn positive_integer_parsers_reject_non_numeric_input() {
        assert_eq!(
            positive_u32("not-a-number").unwrap_err(),
            "'not-a-number' is not a valid non-negative integer"
        );
        assert_eq!(
            positive_u64("not-a-number").unwrap_err(),
            "'not-a-number' is not a valid non-negative integer"
        );
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
    fn process_type_converts_back_to_every_arg_variant() {
        assert_eq!(
            ProcessTypeArg::from(bhtune_core::ProcessType::Flow),
            ProcessTypeArg::Flow
        );
        assert_eq!(
            ProcessTypeArg::from(bhtune_core::ProcessType::PressureLine),
            ProcessTypeArg::PressureLine
        );
        assert_eq!(
            ProcessTypeArg::from(bhtune_core::ProcessType::PressureVessel),
            ProcessTypeArg::PressureVessel
        );
        assert_eq!(
            ProcessTypeArg::from(bhtune_core::ProcessType::Level),
            ProcessTypeArg::Level
        );
        assert_eq!(
            ProcessTypeArg::from(bhtune_core::ProcessType::TemperatureMixing),
            ProcessTypeArg::TemperatureMixing
        );
        assert_eq!(
            ProcessTypeArg::from(bhtune_core::ProcessType::TemperatureHeatExchange),
            ProcessTypeArg::TemperatureHeatExchange
        );
    }

    #[test]
    fn controller_type_converts_back_to_every_arg_variant() {
        assert_eq!(
            ControllerTypeArg::from(bhtune_core::ControllerType::P),
            ControllerTypeArg::P
        );
        assert_eq!(
            ControllerTypeArg::from(bhtune_core::ControllerType::Pi),
            ControllerTypeArg::Pi
        );
        assert_eq!(
            ControllerTypeArg::from(bhtune_core::ControllerType::Pid),
            ControllerTypeArg::Pid
        );
    }

    #[test]
    fn direction_converts_back_to_every_arg_variant() {
        assert_eq!(
            DirectionArg::from(bhtune_core::ControllerDirection::Direct),
            DirectionArg::Direct
        );
        assert_eq!(
            DirectionArg::from(bhtune_core::ControllerDirection::Reverse),
            DirectionArg::Reverse
        );
    }

    #[test]
    fn driver_kind_arg_try_from_tune_driver_covers_the_implemented_drivers() {
        assert_eq!(
            DriverKindArg::try_from(bhtune_db::models::TuneDriver::Opcda).unwrap(),
            DriverKindArg::Opcda
        );
        assert_eq!(
            DriverKindArg::try_from(bhtune_db::models::TuneDriver::Simulator).unwrap(),
            DriverKindArg::Simulator
        );
    }

    #[test]
    fn driver_kind_arg_try_from_tune_driver_rejects_replay() {
        let err = DriverKindArg::try_from(bhtune_db::models::TuneDriver::Replay).unwrap_err();
        assert_eq!(err, ReplayDriverUnsupported);
        assert!(err.to_string().contains("replay"));
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
    fn simulate_args_expand_into_tune_args_with_simulator_driver() {
        let cli = Cli::parse_from(["bhtune", "simulate"]);
        let simulate = expect_variant!(cli.command, Command::Simulate(s) => s, "Simulate");
        let tune = simulate.into_tune_args();
        assert!(matches!(tune.driver, DriverKindArg::Simulator));
        assert_eq!(tune.tagname, "Sim.Loop1.PV");
        assert_eq!(tune.pv_range_low, Some(0.0));
        assert_eq!(tune.pv_range_high, Some(100.0));
        assert_eq!(tune.mv_range_low, Some(0.0));
        assert_eq!(tune.mv_range_high, Some(100.0));
        assert!(matches!(tune.direction, Some(DirectionArg::Reverse)));
        assert!(!tune.yes);
        assert!(tune.write_pid.is_none());
        assert_eq!(tune.op_timeout_secs, 30);
        assert_eq!(tune.restore_timeout_secs, 30);
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
    fn response_level_converts_back_to_every_arg_variant() {
        assert_eq!(
            ResponseLevelArg::from(bhtune_core::ResponseLevel::Aggressive),
            ResponseLevelArg::Aggressive
        );
        assert_eq!(
            ResponseLevelArg::from(bhtune_core::ResponseLevel::Moderate),
            ResponseLevelArg::Moderate
        );
        assert_eq!(
            ResponseLevelArg::from(bhtune_core::ResponseLevel::Sluggish),
            ResponseLevelArg::Sluggish
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
            "--driver",
            "simulator",
        ]);
        let args = expect_variant!(cli.command, Command::Tune(a) => a, "Tune");
        assert_eq!(args.tagname, "Unit1.LIC101.PV");
        assert!(matches!(args.process_type, ProcessTypeArg::Flow));
        assert!(matches!(args.controller_type, ControllerTypeArg::Pi));
        assert!(matches!(args.driver, DriverKindArg::Simulator));
        assert_eq!(args.poll_interval_ms, 800);
        assert!(!args.yes);
        assert!(args.write_pid.is_none());
        assert_eq!(args.op_timeout_secs, 30);
        assert_eq!(args.restore_timeout_secs, 30);
        assert_eq!(args.output, crate::output::OutputFormat::Table);
    }

    #[test]
    fn cli_parses_tune_op_and_restore_timeout_flags() {
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
            "--driver",
            "simulator",
            "--op-timeout-secs",
            "15",
            "--restore-timeout-secs",
            "45",
        ]);
        let args = expect_variant!(cli.command, Command::Tune(a) => a, "Tune");
        assert_eq!(args.op_timeout_secs, 15);
        assert_eq!(args.restore_timeout_secs, 45);
    }

    #[test]
    fn cli_rejects_zero_op_timeout_secs() {
        let result = Cli::try_parse_from([
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
            "--driver",
            "simulator",
            "--op-timeout-secs",
            "0",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn cli_rejects_zero_restore_timeout_secs() {
        let result = Cli::try_parse_from([
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
            "--driver",
            "simulator",
            "--restore-timeout-secs",
            "0",
        ]);
        assert!(result.is_err());
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
            "--driver",
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
    fn cli_parses_history_prune_defaults() {
        let cli = Cli::parse_from(["bhtune", "history", "prune"]);
        let command =
            expect_variant!(cli.command, Command::History { command } => command, "History");
        let (older_than_days, dry_run, output) = expect_variant!(
            command,
            HistoryCommand::Prune { older_than_days, dry_run, output } => (older_than_days, dry_run, output),
            "Prune"
        );
        assert_eq!(older_than_days, None);
        assert!(!dry_run);
        assert_eq!(output, crate::output::OutputFormat::Table);
    }

    #[test]
    fn cli_parses_history_prune_with_older_than_days_and_dry_run() {
        let cli = Cli::parse_from([
            "bhtune",
            "history",
            "prune",
            "--older-than-days",
            "14",
            "--dry-run",
            "--output",
            "json",
        ]);
        let command =
            expect_variant!(cli.command, Command::History { command } => command, "History");
        let (older_than_days, dry_run, output) = expect_variant!(
            command,
            HistoryCommand::Prune { older_than_days, dry_run, output } => (older_than_days, dry_run, output),
            "Prune"
        );
        assert_eq!(older_than_days, Some(14));
        assert!(dry_run);
        assert_eq!(output, crate::output::OutputFormat::Json);
    }

    #[test]
    fn cli_rejects_a_zero_history_prune_older_than_days() {
        let result = Cli::try_parse_from(["bhtune", "history", "prune", "--older-than-days", "0"]);
        assert!(result.is_err());
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

        let cli = Cli::parse_from(["bhtune", "simulate", "--output", "json"]);
        assert_eq!(
            cli.command.output_format(),
            crate::output::OutputFormat::Json
        );
    }

    #[test]
    fn cli_rejects_missing_required_tune_flags() {
        let result = Cli::try_parse_from(["bhtune", "tune"]);
        assert!(result.is_err());
    }

    #[test]
    fn cli_db_config_and_templates_default_to_none() {
        let cli = Cli::parse_from(["bhtune", "simulate"]);
        assert_eq!(cli.db, None);
        assert_eq!(cli.config, None);
        assert_eq!(cli.templates, None);
        assert_eq!(cli.retention_days, None);
    }

    #[test]
    fn cli_parses_explicit_db_config_and_templates_flags() {
        let cli = Cli::parse_from([
            "bhtune",
            "--db",
            "/data/bhtune.db",
            "--config",
            "/etc/bhtune.toml",
            "--templates",
            "/etc/bhtune/templates.toml",
            "--retention-days",
            "30",
            "simulate",
        ]);
        assert_eq!(cli.db, Some(PathBuf::from("/data/bhtune.db")));
        assert_eq!(cli.config, Some(PathBuf::from("/etc/bhtune.toml")));
        assert_eq!(
            cli.templates,
            Some(PathBuf::from("/etc/bhtune/templates.toml"))
        );
        assert_eq!(cli.retention_days, Some(30));
    }

    #[test]
    fn cli_rejects_a_zero_retention_days() {
        // `positive_u32` -- `0` is a nonsensical "delete everything immediately" policy, not
        // a legitimate "keep nothing older than zero days" configuration.
        let result = Cli::try_parse_from(["bhtune", "--retention-days", "0", "simulate"]);
        assert!(result.is_err());
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
            "--driver",
            "simulator",
        ]);
        let args = expect_variant!(cli.command, Command::Tune(a) => a, "Tune");
        assert_eq!(args.bridge_host, None);
    }

    #[test]
    fn opc_servers_bridge_host_defaults_to_none() {
        let cli = Cli::parse_from(["bhtune", "opc", "servers"]);
        let command = expect_variant!(cli.command, Command::Opc { command } => command, "Opc");
        let bridge_host =
            expect_variant!(command, OpcCommand::Servers { bridge_host } => bridge_host, "Servers");
        assert_eq!(bridge_host, None);
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
