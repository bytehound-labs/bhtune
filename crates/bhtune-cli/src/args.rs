//! Command-line argument definitions (`clap` derive) and the small wrapper enums that adapt
//! `bhtune-core`'s domain enums to `clap::ValueEnum`.
//!
//! Rust's orphan rule forbids implementing a foreign trait (`clap::ValueEnum`) for a foreign
//! type (`bhtune_core::ProcessType` etc.), so each domain enum this CLI exposes as a flag
//! gets a small local wrapper here with a `From`/`Into` conversion — not a design choice, a
//! language requirement.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// The `bhtune` CLI: a scriptable, no-GUI way to run an MRFT tune and inspect its history.
#[derive(Parser, Debug)]
#[command(name = "bhtune", version, about = "Headless MRFT auto-tuner")]
pub struct Cli {
    /// Path to the SQLite database file.
    ///
    /// A simple, hardcoded-default placeholder — platform-standard data directories and
    /// CLI > env > TOML > default precedence land with the separate `cli-config` phase; this
    /// flag is the only way to configure the DB path until then.
    #[arg(long, global = true, default_value = "bhtune.db")]
    pub db: PathBuf,

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
    #[arg(long)]
    pub relay_amp: f32,

    /// Relay cycles to skip before counting begins (default: looked up per `--process-type`).
    #[arg(long)]
    pub cycles_skip: Option<u32>,

    /// Relay cycles to count once the skip period ends (default: looked up per
    /// `--process-type`).
    #[arg(long)]
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
    /// DCOM host directly — see AGENTS.md's OPC DA integration notes. Required (and only
    /// meaningful) with `--backend opcda`.
    #[arg(long, default_value = "localhost:7600")]
    pub bridge_host: String,

    /// OPC DA server ProgID (legacy: `-s`/`--opcServerID`). Required with `--backend opcda`.
    #[arg(long)]
    pub server: Option<String>,

    /// Simulator process gain (`--backend simulator` only).
    #[arg(long, default_value_t = 1.0)]
    pub sim_gain: f32,
    /// Simulator process time constant, in seconds (`--backend simulator` only).
    #[arg(long, default_value_t = 2.0)]
    pub sim_tau: f32,
    /// Simulator dead time, in seconds (`--backend simulator` only).
    #[arg(long, default_value_t = 5.0)]
    pub sim_dead_time: f32,
    /// Simulator measurement noise amplitude (`--backend simulator` only).
    #[arg(long, default_value_t = 0.0)]
    pub sim_noise: f32,
    /// Simulator RNG seed, for reproducible noise (`--backend simulator` only).
    #[arg(long, default_value_t = 0)]
    pub sim_seed: u64,
    /// Simulator initial PV (`--backend simulator` only).
    #[arg(long, default_value_t = 50.0)]
    pub sim_initial_pv: f32,
    /// Simulator initial MV (`--backend simulator` only).
    #[arg(long, default_value_t = 50.0)]
    pub sim_initial_mv: f32,

    /// Fixed PV range high, overriding a live tag read (legacy: the PV range "toggle
    /// tag/value" button). Required (defaults to 100.0) for `--backend simulator`, which has
    /// no range tags at all.
    #[arg(long)]
    pub pv_range_high: Option<f32>,
    /// Fixed PV range low, overriding a live tag read.
    #[arg(long)]
    pub pv_range_low: Option<f32>,
    /// Fixed MV range high, overriding a live tag read.
    #[arg(long)]
    pub mv_range_high: Option<f32>,
    /// Fixed MV range low, overriding a live tag read.
    #[arg(long)]
    pub mv_range_low: Option<f32>,
    /// Fixed controller direction, overriding a live tag read.
    #[arg(long, value_enum)]
    pub direction: Option<DirectionArg>,

    /// How often to poll the backend, in milliseconds (legacy: the 800 ms WinForms timer).
    #[arg(long, default_value_t = 800)]
    pub poll_interval_ms: u64,

    /// A friendly name for this run, recorded as `loop_name` (default: the PV tag name).
    #[arg(long)]
    pub name: Option<String>,
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

    #[arg(long, default_value_t = 10.0)]
    pub relay_amp: f32,

    #[arg(long)]
    pub cycles_skip: Option<u32>,
    #[arg(long)]
    pub cycles_count: Option<u32>,
    #[arg(long)]
    pub noise_protection_secs: Option<u32>,
    #[arg(long, default_value_t = 0)]
    pub mrft_delay: u32,

    #[arg(long, default_value_t = 1.0)]
    pub sim_gain: f32,
    #[arg(long, default_value_t = 2.0)]
    pub sim_tau: f32,
    #[arg(long, default_value_t = 5.0)]
    pub sim_dead_time: f32,
    #[arg(long, default_value_t = 0.0)]
    pub sim_noise: f32,
    #[arg(long, default_value_t = 0)]
    pub sim_seed: u64,
    #[arg(long, default_value_t = 50.0)]
    pub sim_initial_pv: f32,
    #[arg(long, default_value_t = 50.0)]
    pub sim_initial_mv: f32,

    #[arg(long, default_value_t = 800)]
    pub poll_interval_ms: u64,

    #[arg(long)]
    pub name: Option<String>,
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
            bridge_host: String::new(),
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
            name: self.name,
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
    },
    /// Show one run's full detail: config, initial readings, calculated results, and any
    /// PID write-back audit rows.
    Show { run_id: i64 },
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
        #[arg(long, default_value = "localhost:7600")]
        bridge_host: String,
        #[arg(long)]
        server: String,
        tags: Vec<String>,
    },
    /// Write a value to one tag.
    Write {
        #[arg(long, default_value = "localhost:7600")]
        bridge_host: String,
        #[arg(long)]
        server: String,
        tag: String,
        value: String,
    },
    /// Browse tags under a path (empty for the top level).
    Browse {
        #[arg(long, default_value = "localhost:7600")]
        bridge_host: String,
        #[arg(long)]
        server: String,
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
        let (outcome, limit, offset) = expect_variant!(
            command,
            HistoryCommand::List { outcome, limit, offset } => (outcome, limit, offset),
            "List"
        );
        assert!(matches!(outcome, Some(OutcomeArg::Completed)));
        assert_eq!(limit, 10);
        assert_eq!(offset, 0);
    }

    #[test]
    fn cli_rejects_missing_required_tune_flags() {
        let result = Cli::try_parse_from(["bhtune", "tune"]);
        assert!(result.is_err());
    }
}
