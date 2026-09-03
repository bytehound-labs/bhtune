//! Global bhtune configuration, including the shared `[tuning]` timing policy and the
//! `CLI flag > env var > TOML config file > built-in default` precedence used by settings
//! that expose command-line or environment overrides.

use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
    net::IpAddr,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Default opcda-bridge gateway address bhtune connects to when nothing else specifies one.
pub const DEFAULT_BRIDGE_HOST: &str = "localhost:7600";

/// Default address `bhtune-server` binds to when nothing else specifies one -- loopback
/// only, matching the "v1 binds to `127.0.0.1` by default" decision in AGENTS.md. Lives
/// alongside [`DEFAULT_BRIDGE_HOST`] in this shared config module (rather than in
/// `bhtune-server` itself) even though only the server binary ever calls
/// [`resolve_bind_addr`], the same way `templates`/`log` below are settings only some
/// commands consume -- one `bhtune.toml` file and one precedence chain for every bhtune
/// setting, CLI or server.
pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8787";

/// Built-in pre/post-MRFT recording padding when `[tuning].mrft_delay_secs` is absent.
pub const DEFAULT_TUNING_MRFT_DELAY_SECS: u32 = 0;
/// Built-in driver polling interval when `[tuning].poll_interval_ms` is absent.
pub const DEFAULT_TUNING_POLL_INTERVAL_MS: u64 = 800;
/// Built-in whole-run timeout when `[tuning].timeout_secs` is absent.
pub const DEFAULT_TUNING_TIMEOUT_SECS: u64 = 3_600;
/// Built-in per-driver-operation timeout when `[tuning].op_timeout_secs` is absent.
pub const DEFAULT_TUNING_OP_TIMEOUT_SECS: u64 = 30;
/// Built-in post-run restoration timeout when `[tuning].restore_timeout_secs` is absent.
pub const DEFAULT_TUNING_RESTORE_TIMEOUT_SECS: u64 = 30;
/// Largest supported pre/post-MRFT recording delay.
pub const MAX_TUNING_MRFT_DELAY_SECS: u32 = 3_600;
/// Minimum restoration timeout for OPC DA runs.
pub const MIN_OPC_RESTORE_TIMEOUT_SECS: u64 = 4;
/// Fixed lifetime of an anonymous public Demo visitor session.
pub const DEMO_SESSION_TTL_SECS: u64 = 86_400;
/// Fixed simulator polling interval used by public Demo tunes.
pub const DEMO_POLL_INTERVAL_MS: u64 = 200;
/// Fixed whole-run timeout used by public Demo tunes.
pub const DEMO_RUN_TIMEOUT_SECS: u64 = 30;
/// Maximum number of Demo tunes that may be active across all visitors.
pub const DEMO_MAX_ACTIVE_RUNS_GLOBAL: u32 = 8;
/// Maximum number of Demo tunes that may be active for one visitor.
pub const DEMO_MAX_ACTIVE_RUNS_PER_VISITOR: u32 = 1;
/// Accepted Demo tune starts allowed for one session token in a quota window.
pub const DEMO_ACCEPTED_STARTS_PER_TOKEN: u32 = 6;
/// Accepted Demo tune starts allowed for one client IP in a quota window.
pub const DEMO_ACCEPTED_STARTS_PER_CLIENT_IP: u32 = 6;
/// Fixed window used by the accepted-start quotas.
pub const DEMO_ACCEPTED_START_WINDOW_SECS: u64 = 600;
/// Maximum accepted demo runs retained per session, independent of active-run concurrency.
pub const DEMO_MAX_RUNS_PER_SESSION: u32 = 10;
/// Maximum number of completed Demo runs retained for one visitor.
pub const DEMO_RETAINED_RUNS_PER_VISITOR: u32 = 10;
/// Maximum number of current Demo-owned `tune_runs` rows in the database.
pub const DEMO_MAX_TUNE_RUN_ROWS_GLOBAL: u32 = 5_000;
/// Maximum JSON request-body size accepted by the Demo API.
pub const DEMO_MAX_JSON_BODY_BYTES: u64 = 32_768;
/// Maximum number of simultaneous SSE streams for one visitor.
pub const DEMO_MAX_SSE_PER_VISITOR: u32 = 2;
/// Maximum number of simultaneous Demo SSE streams across all visitors.
pub const DEMO_MAX_SSE_GLOBAL: u32 = 32;
/// Absolute lifetime of one Demo SSE stream.
pub const DEMO_SSE_LIFETIME_SECS: u64 = 45;
/// Maximum number of ordinary Demo API requests processed concurrently.
pub const DEMO_ORDINARY_REQUEST_CONCURRENCY: u32 = 64;
/// Timeout applied to an ordinary, non-streaming Demo API request.
pub const DEMO_ORDINARY_REQUEST_TIMEOUT_SECS: u64 = 10;
/// Interval between expired-session and excess-history cleanup passes.
pub const DEMO_CLEANUP_INTERVAL_SECS: u64 = 300;
/// Built-in template exposed by the public Demo.
pub const DEMO_TEMPLATE_NAME: &str = "Yokogawa CentumVP";
/// Fixed persisted/display label used by public Demo runs.
///
/// This is deliberately not an OPC or simulator driver tag. The simulator still uses its
/// internal `Sim.PV`/`Sim.MV` tags; this value is the stable run identity shown to visitors.
pub const DEMO_TAG_NAME: &str = "Simulator demo";
/// Default PV and MV lower bound used by public Demo runs.
pub const DEMO_RANGE_LOW: f32 = 0.0;
/// Default PV and MV upper bound used by public Demo runs.
pub const DEMO_RANGE_HIGH: f32 = 100.0;
/// Minimum PV/MV range endpoint accepted by the public Demo.
pub const DEMO_RANGE_ENDPOINT_MIN: f32 = -1_000.0;
/// Maximum PV/MV range endpoint accepted by the public Demo.
pub const DEMO_RANGE_ENDPOINT_MAX: f32 = 1_000.0;
/// Minimum PV/MV span accepted by the public Demo.
pub const DEMO_RANGE_SPAN_MIN: f32 = 1.0;
/// Maximum PV/MV span accepted by the public Demo.
pub const DEMO_RANGE_SPAN_MAX: f32 = 1_000.0;
/// Minimum non-zero process-gain magnitude accepted by the public Demo.
pub const DEMO_SIM_GAIN_ABS_MIN: f32 = 0.1;
/// Maximum process-gain magnitude accepted by the public Demo.
pub const DEMO_SIM_GAIN_MAX: f32 = 5.0;
/// Minimum simulator time constant accepted by the public Demo.
pub const DEMO_SIM_TAU_MIN: f32 = 0.05;
/// Maximum simulator time constant accepted by the public Demo.
pub const DEMO_SIM_TAU_MAX: f32 = 5.0;
/// Minimum simulator dead time accepted by the public Demo.
pub const DEMO_SIM_DEAD_TIME_MIN: f32 = 0.0;
/// Maximum simulator dead time accepted by the public Demo.
pub const DEMO_SIM_DEAD_TIME_MAX: f32 = 2.0;
/// Minimum simulator noise amplitude accepted by the public Demo.
pub const DEMO_SIM_NOISE_MIN: f32 = 0.0;
/// Maximum simulator noise amplitude as a fraction of the configured PV span.
pub const DEMO_SIM_NOISE_MAX_PV_SPAN_FRACTION: f32 = 0.05;
/// Largest simulator seed accepted by the public Demo.
pub const DEMO_SIM_SEED_MAX: u64 = i32::MAX as u64;
/// Minimum relay amplitude accepted by the public Demo.
pub const DEMO_RELAY_AMP_MIN: f32 = 1.0;
/// Maximum relay amplitude accepted by the public Demo.
pub const DEMO_RELAY_AMP_MAX: f32 = 20.0;
/// Default relay amplitude used by the Demo form.
pub const DEMO_RELAY_AMP_DEFAULT: f32 = 10.0;
/// Minimum relay-cycle skip count accepted by the public Demo.
pub const DEMO_CYCLES_SKIP_MIN: u32 = 0;
/// Maximum relay-cycle skip count accepted by the public Demo.
pub const DEMO_CYCLES_SKIP_MAX: u32 = 2;
/// Default relay-cycle skip count used by the Demo form.
pub const DEMO_CYCLES_SKIP_DEFAULT: u32 = 1;
/// Minimum relay-cycle count accepted by the public Demo.
pub const DEMO_CYCLES_COUNT_MIN: u32 = 1;
/// Maximum relay-cycle count accepted by the public Demo.
pub const DEMO_CYCLES_COUNT_MAX: u32 = 3;
/// Default relay-cycle count used by the Demo form.
pub const DEMO_CYCLES_COUNT_DEFAULT: u32 = 2;
/// Minimum switch noise-protection delay accepted by the public Demo.
pub const DEMO_NOISE_PROTECTION_SECS_MIN: u32 = 0;
/// Maximum switch noise-protection delay accepted by the public Demo.
pub const DEMO_NOISE_PROTECTION_SECS_MAX: u32 = 3;
/// Default switch noise-protection delay used by the Demo form.
pub const DEMO_NOISE_PROTECTION_SECS_DEFAULT: u32 = 0;
/// Default simulator process gain used by the Demo form.
pub const DEMO_SIM_GAIN_DEFAULT: f32 = 1.0;
/// Default simulator time constant used by the Demo form.
pub const DEMO_SIM_TAU_DEFAULT: f32 = 0.5;
/// Default simulator dead time used by the Demo form.
pub const DEMO_SIM_DEAD_TIME_DEFAULT: f32 = 1.0;
/// Default simulator noise amplitude used by the Demo form.
pub const DEMO_SIM_NOISE_DEFAULT: f32 = 0.0;
/// Default simulator random seed used by the Demo form.
pub const DEMO_SIM_SEED_DEFAULT: u64 = 0;
/// Default simulator PV and MV starting value used by the Demo form.
pub const DEMO_SIM_INITIAL_VALUE_DEFAULT: f32 = 50.0;
/// Name of the host-only anonymous Demo session cookie.
pub const DEMO_COOKIE_NAME: &str = "__Host-bhtune_demo_session";

/// Runtime server exposure mode. Full mode preserves the normal live-plant API; Demo mode
/// is an explicitly restricted, simulator-only surface intended for public demonstrations.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default, utoipa::ToSchema)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum ServerMode {
    #[default]
    Full,
    Demo,
}

/// Limits applied to the public simulator-only demo surface.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, utoipa::ToSchema)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct DemoPolicy {
    /// Anonymous visitor-session lifetime.
    #[schema(minimum = 86_400, maximum = 86_400)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 86_400, max = 86_400)))]
    pub session_ttl_secs: u64,
    /// Simulator polling interval.
    #[schema(minimum = 200, maximum = 200)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 200, max = 200)))]
    pub poll_interval_ms: u64,
    /// Whole-run timeout.
    #[schema(minimum = 30, maximum = 30)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 30, max = 30)))]
    pub run_timeout_secs: u64,
    /// Active Demo tune limit across all visitors.
    #[schema(minimum = 8, maximum = 8)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 8, max = 8)))]
    pub max_active_runs_global: u32,
    /// Active Demo tune limit for one visitor.
    #[schema(minimum = 1, maximum = 1)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 1, max = 1)))]
    pub max_active_runs_per_visitor: u32,
    /// Accepted starts for one session token in the quota window.
    #[schema(minimum = 6, maximum = 6)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 6, max = 6)))]
    pub accepted_starts_per_token: u32,
    /// Accepted starts for one client IP in the quota window.
    #[schema(minimum = 6, maximum = 6)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 6, max = 6)))]
    pub accepted_starts_per_client_ip: u32,
    /// Window shared by both accepted-start quotas.
    #[schema(minimum = 600, maximum = 600)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 600, max = 600)))]
    pub accepted_start_window_secs: u64,
    /// Completed runs retained for one visitor.
    #[schema(minimum = 10, maximum = 10)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 10, max = 10)))]
    pub retained_runs_per_visitor: u32,
    /// Maximum total demo runs accepted for one visitor.
    pub max_runs_per_session: u32,
    /// Current Demo-owned `tune_runs` row limit across all visitors.
    #[schema(minimum = 5_000, maximum = 5_000)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 5_000, max = 5_000)))]
    pub max_tune_run_rows_global: u32,
    /// Maximum JSON request-body size.
    #[schema(minimum = 32_768, maximum = 32_768)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 32_768, max = 32_768)))]
    pub max_json_body_bytes: u64,
    /// Simultaneous SSE streams for one visitor.
    #[schema(minimum = 2, maximum = 2)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 2, max = 2)))]
    pub max_sse_per_visitor: u32,
    /// Simultaneous Demo SSE streams across all visitors.
    #[schema(minimum = 32, maximum = 32)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 32, max = 32)))]
    pub max_sse_global: u32,
    /// Absolute lifetime of one Demo SSE stream.
    #[schema(minimum = 45, maximum = 45)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 45, max = 45)))]
    pub sse_lifetime_secs: u64,
    /// Concurrent ordinary, non-streaming Demo API requests.
    #[schema(minimum = 64, maximum = 64)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 64, max = 64)))]
    pub ordinary_request_concurrency: u32,
    /// Timeout for an ordinary, non-streaming Demo API request.
    #[schema(minimum = 10, maximum = 10)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 10, max = 10)))]
    pub ordinary_request_timeout_secs: u64,
    /// Interval between Demo cleanup passes.
    #[schema(minimum = 300, maximum = 300)]
    #[cfg_attr(feature = "schemars", schemars(range(min = 300, max = 300)))]
    pub cleanup_interval_secs: u64,
}

impl Default for DemoPolicy {
    fn default() -> Self {
        Self {
            session_ttl_secs: DEMO_SESSION_TTL_SECS,
            poll_interval_ms: DEMO_POLL_INTERVAL_MS,
            run_timeout_secs: DEMO_RUN_TIMEOUT_SECS,
            max_active_runs_global: DEMO_MAX_ACTIVE_RUNS_GLOBAL,
            max_active_runs_per_visitor: DEMO_MAX_ACTIVE_RUNS_PER_VISITOR,
            accepted_starts_per_token: DEMO_ACCEPTED_STARTS_PER_TOKEN,
            accepted_starts_per_client_ip: DEMO_ACCEPTED_STARTS_PER_CLIENT_IP,
            accepted_start_window_secs: DEMO_ACCEPTED_START_WINDOW_SECS,
            retained_runs_per_visitor: DEMO_RETAINED_RUNS_PER_VISITOR,
            max_runs_per_session: DEMO_MAX_RUNS_PER_SESSION,
            max_tune_run_rows_global: DEMO_MAX_TUNE_RUN_ROWS_GLOBAL,
            max_json_body_bytes: DEMO_MAX_JSON_BODY_BYTES,
            max_sse_per_visitor: DEMO_MAX_SSE_PER_VISITOR,
            max_sse_global: DEMO_MAX_SSE_GLOBAL,
            sse_lifetime_secs: DEMO_SSE_LIFETIME_SECS,
            ordinary_request_concurrency: DEMO_ORDINARY_REQUEST_CONCURRENCY,
            ordinary_request_timeout_secs: DEMO_ORDINARY_REQUEST_TIMEOUT_SECS,
            cleanup_interval_secs: DEMO_CLEANUP_INTERVAL_SECS,
        }
    }
}

impl DemoPolicy {
    pub fn validate(&self) -> Result<(), String> {
        validate_demo_value(
            "session_ttl_secs",
            self.session_ttl_secs,
            DEMO_SESSION_TTL_SECS,
        )?;
        validate_demo_value(
            "poll_interval_ms",
            self.poll_interval_ms,
            DEMO_POLL_INTERVAL_MS,
        )?;
        validate_demo_value(
            "run_timeout_secs",
            self.run_timeout_secs,
            DEMO_RUN_TIMEOUT_SECS,
        )?;
        validate_demo_value(
            "max_active_runs_global",
            self.max_active_runs_global,
            DEMO_MAX_ACTIVE_RUNS_GLOBAL,
        )?;
        validate_demo_value(
            "max_active_runs_per_visitor",
            self.max_active_runs_per_visitor,
            DEMO_MAX_ACTIVE_RUNS_PER_VISITOR,
        )?;
        validate_demo_value(
            "accepted_starts_per_token",
            self.accepted_starts_per_token,
            DEMO_ACCEPTED_STARTS_PER_TOKEN,
        )?;
        validate_demo_value(
            "accepted_starts_per_client_ip",
            self.accepted_starts_per_client_ip,
            DEMO_ACCEPTED_STARTS_PER_CLIENT_IP,
        )?;
        validate_demo_value(
            "accepted_start_window_secs",
            self.accepted_start_window_secs,
            DEMO_ACCEPTED_START_WINDOW_SECS,
        )?;
        validate_demo_value(
            "retained_runs_per_visitor",
            self.retained_runs_per_visitor,
            DEMO_RETAINED_RUNS_PER_VISITOR,
        )?;
        validate_demo_value(
            "max_runs_per_session",
            self.max_runs_per_session,
            DEMO_MAX_RUNS_PER_SESSION,
        )?;
        validate_demo_value(
            "max_tune_run_rows_global",
            self.max_tune_run_rows_global,
            DEMO_MAX_TUNE_RUN_ROWS_GLOBAL,
        )?;
        validate_demo_value(
            "max_json_body_bytes",
            self.max_json_body_bytes,
            DEMO_MAX_JSON_BODY_BYTES,
        )?;
        validate_demo_value(
            "max_sse_per_visitor",
            self.max_sse_per_visitor,
            DEMO_MAX_SSE_PER_VISITOR,
        )?;
        validate_demo_value("max_sse_global", self.max_sse_global, DEMO_MAX_SSE_GLOBAL)?;
        validate_demo_value(
            "sse_lifetime_secs",
            self.sse_lifetime_secs,
            DEMO_SSE_LIFETIME_SECS,
        )?;
        validate_demo_value(
            "ordinary_request_concurrency",
            self.ordinary_request_concurrency,
            DEMO_ORDINARY_REQUEST_CONCURRENCY,
        )?;
        validate_demo_value(
            "ordinary_request_timeout_secs",
            self.ordinary_request_timeout_secs,
            DEMO_ORDINARY_REQUEST_TIMEOUT_SECS,
        )?;
        validate_demo_value(
            "cleanup_interval_secs",
            self.cleanup_interval_secs,
            DEMO_CLEANUP_INTERVAL_SECS,
        )?;
        Ok(())
    }
}

fn validate_demo_value<T>(field: &str, actual: T, expected: T) -> Result<(), String>
where
    T: PartialEq + std::fmt::Display,
{
    if actual == expected {
        Ok(())
    } else {
        Err(format!("demo.{field} must be exactly {expected}"))
    }
}

/// Optional declarations of the fixed [`DemoPolicy`] contract.
///
/// Missing keys receive the approved value. A present key must state that same value; public
/// Demo deployments cannot weaken or silently diverge from the documented resource policy.
#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, utoipa::ToSchema)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DemoPolicyConfig {
    /// Anonymous visitor-session lifetime. Fixed at 86,400 seconds.
    #[cfg_attr(feature = "schemars", schemars(range(min = 86_400, max = 86_400)))]
    pub session_ttl_secs: Option<u64>,
    /// Simulator polling interval. Fixed at 200 milliseconds.
    #[cfg_attr(feature = "schemars", schemars(range(min = 200, max = 200)))]
    pub poll_interval_ms: Option<u64>,
    /// Whole-run timeout. Fixed at 30 seconds.
    #[cfg_attr(feature = "schemars", schemars(range(min = 30, max = 30)))]
    pub run_timeout_secs: Option<u64>,
    /// Active Demo tune limit across all visitors. Fixed at 8.
    #[cfg_attr(feature = "schemars", schemars(range(min = 8, max = 8)))]
    pub max_active_runs_global: Option<u32>,
    /// Active Demo tune limit for one visitor. Fixed at 1.
    #[cfg_attr(feature = "schemars", schemars(range(min = 1, max = 1)))]
    pub max_active_runs_per_visitor: Option<u32>,
    /// Accepted starts for one session token in the quota window. Fixed at 6.
    #[cfg_attr(feature = "schemars", schemars(range(min = 6, max = 6)))]
    pub accepted_starts_per_token: Option<u32>,
    /// Accepted starts for one client IP in the quota window. Fixed at 6.
    #[cfg_attr(feature = "schemars", schemars(range(min = 6, max = 6)))]
    pub accepted_starts_per_client_ip: Option<u32>,
    /// Window shared by both accepted-start quotas. Fixed at 600 seconds.
    #[cfg_attr(feature = "schemars", schemars(range(min = 600, max = 600)))]
    pub accepted_start_window_secs: Option<u64>,
    /// Completed runs retained for one visitor. Fixed at 10.
    #[cfg_attr(feature = "schemars", schemars(range(min = 10, max = 10)))]
    pub retained_runs_per_visitor: Option<u32>,
    pub max_runs_per_session: Option<u32>,
    /// Current Demo-owned `tune_runs` row limit across all visitors. Fixed at 5,000.
    #[cfg_attr(feature = "schemars", schemars(range(min = 5_000, max = 5_000)))]
    pub max_tune_run_rows_global: Option<u32>,
    /// Maximum JSON request-body size. Fixed at 32,768 bytes.
    #[cfg_attr(feature = "schemars", schemars(range(min = 32_768, max = 32_768)))]
    pub max_json_body_bytes: Option<u64>,
    /// Simultaneous SSE streams for one visitor. Fixed at 2.
    #[cfg_attr(feature = "schemars", schemars(range(min = 2, max = 2)))]
    pub max_sse_per_visitor: Option<u32>,
    /// Simultaneous Demo SSE streams across all visitors. Fixed at 32.
    #[cfg_attr(feature = "schemars", schemars(range(min = 32, max = 32)))]
    pub max_sse_global: Option<u32>,
    /// Absolute lifetime of one Demo SSE stream. Fixed at 45 seconds.
    #[cfg_attr(feature = "schemars", schemars(range(min = 45, max = 45)))]
    pub sse_lifetime_secs: Option<u64>,
    /// Concurrent ordinary, non-streaming Demo API requests. Fixed at 64.
    #[cfg_attr(feature = "schemars", schemars(range(min = 64, max = 64)))]
    pub ordinary_request_concurrency: Option<u32>,
    /// Timeout for an ordinary, non-streaming Demo API request. Fixed at 10 seconds.
    #[cfg_attr(feature = "schemars", schemars(range(min = 10, max = 10)))]
    pub ordinary_request_timeout_secs: Option<u64>,
    /// Interval between Demo cleanup passes. Fixed at 300 seconds.
    #[cfg_attr(feature = "schemars", schemars(range(min = 300, max = 300)))]
    pub cleanup_interval_secs: Option<u64>,
}

pub fn resolve_demo_policy(config: &DemoPolicyConfig) -> Result<DemoPolicy, String> {
    let defaults = DemoPolicy::default();
    let policy = DemoPolicy {
        session_ttl_secs: config.session_ttl_secs.unwrap_or(defaults.session_ttl_secs),
        poll_interval_ms: config.poll_interval_ms.unwrap_or(defaults.poll_interval_ms),
        run_timeout_secs: config.run_timeout_secs.unwrap_or(defaults.run_timeout_secs),
        max_active_runs_global: config
            .max_active_runs_global
            .unwrap_or(defaults.max_active_runs_global),
        max_active_runs_per_visitor: config
            .max_active_runs_per_visitor
            .unwrap_or(defaults.max_active_runs_per_visitor),
        accepted_starts_per_token: config
            .accepted_starts_per_token
            .unwrap_or(defaults.accepted_starts_per_token),
        accepted_starts_per_client_ip: config
            .accepted_starts_per_client_ip
            .unwrap_or(defaults.accepted_starts_per_client_ip),
        accepted_start_window_secs: config
            .accepted_start_window_secs
            .unwrap_or(defaults.accepted_start_window_secs),
        retained_runs_per_visitor: config
            .retained_runs_per_visitor
            .unwrap_or(defaults.retained_runs_per_visitor),
        max_runs_per_session: config
            .max_runs_per_session
            .unwrap_or(defaults.max_runs_per_session),
        max_tune_run_rows_global: config
            .max_tune_run_rows_global
            .unwrap_or(defaults.max_tune_run_rows_global),
        max_json_body_bytes: config
            .max_json_body_bytes
            .unwrap_or(defaults.max_json_body_bytes),
        max_sse_per_visitor: config
            .max_sse_per_visitor
            .unwrap_or(defaults.max_sse_per_visitor),
        max_sse_global: config.max_sse_global.unwrap_or(defaults.max_sse_global),
        sse_lifetime_secs: config
            .sse_lifetime_secs
            .unwrap_or(defaults.sse_lifetime_secs),
        ordinary_request_concurrency: config
            .ordinary_request_concurrency
            .unwrap_or(defaults.ordinary_request_concurrency),
        ordinary_request_timeout_secs: config
            .ordinary_request_timeout_secs
            .unwrap_or(defaults.ordinary_request_timeout_secs),
        cleanup_interval_secs: config
            .cleanup_interval_secs
            .unwrap_or(defaults.cleanup_interval_secs),
    };
    policy.validate()?;
    Ok(policy)
}

pub fn resolve_server_mode(
    env_mode: Option<&str>,
    config: &BhtuneConfig,
) -> Result<ServerMode, String> {
    let raw = env_mode.map(str::to_owned).or_else(|| {
        config.server_mode.map(|mode| match mode {
            ServerMode::Full => "full".to_owned(),
            ServerMode::Demo => "demo".to_owned(),
        })
    });
    match raw
        .as_deref()
        .unwrap_or("full")
        .to_ascii_lowercase()
        .as_str()
    {
        "full" => Ok(ServerMode::Full),
        "demo" => Ok(ServerMode::Demo),
        other => Err(format!(
            "invalid server mode '{other}'; expected 'full' or 'demo'"
        )),
    }
}

pub fn resolve_demo_policy_from_config(config: &BhtuneConfig) -> Result<DemoPolicy, String> {
    resolve_demo_policy(&config.demo)
}

/// Optional values authored in the `[tuning]` table.
///
/// Missing keys stay `None` so callers can distinguish an explicit TOML value from a
/// built-in default. Use [`resolve_tuning_config`] to obtain the concrete values used by a
/// tune and [`validate_tuning_config`] before preparing the run.
#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct TuningConfig {
    #[cfg_attr(feature = "schemars", schemars(range(max = 3_600)))]
    pub mrft_delay_secs: Option<u32>,
    #[cfg_attr(feature = "schemars", schemars(range(min = 1)))]
    pub poll_interval_ms: Option<u64>,
    #[cfg_attr(feature = "schemars", schemars(range(min = 1)))]
    pub timeout_secs: Option<u64>,
    #[cfg_attr(feature = "schemars", schemars(range(min = 1)))]
    pub op_timeout_secs: Option<u64>,
    #[cfg_attr(feature = "schemars", schemars(range(min = 1)))]
    pub restore_timeout_secs: Option<u64>,
}

/// Concrete tuning timing policy after absent TOML keys have received built-in defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveTuningConfig {
    pub mrft_delay_secs: u32,
    pub poll_interval_ms: u64,
    pub timeout_secs: u64,
    pub op_timeout_secs: u64,
    pub restore_timeout_secs: u64,
}

impl Default for EffectiveTuningConfig {
    fn default() -> Self {
        Self {
            mrft_delay_secs: DEFAULT_TUNING_MRFT_DELAY_SECS,
            poll_interval_ms: DEFAULT_TUNING_POLL_INTERVAL_MS,
            timeout_secs: DEFAULT_TUNING_TIMEOUT_SECS,
            op_timeout_secs: DEFAULT_TUNING_OP_TIMEOUT_SECS,
            restore_timeout_secs: DEFAULT_TUNING_RESTORE_TIMEOUT_SECS,
        }
    }
}

/// Origin of one effective tuning value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuningConfigSource {
    Toml,
    BuiltInDefault,
}

/// Per-field provenance for the effective `[tuning]` policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TuningConfigSources {
    pub mrft_delay_secs: TuningConfigSource,
    pub poll_interval_ms: TuningConfigSource,
    pub timeout_secs: TuningConfigSource,
    pub op_timeout_secs: TuningConfigSource,
    pub restore_timeout_secs: TuningConfigSource,
}

/// Validation error for concrete tuning timing values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuningConfigError {
    MrftDelayOutOfRange { value: u32 },
    PollIntervalTooSmall { value: u64 },
    TimeoutTooSmall { value: u64 },
    OpTimeoutTooSmall { value: u64 },
    RestoreTimeoutTooSmall { value: u64, minimum: u64 },
}

impl std::fmt::Display for TuningConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MrftDelayOutOfRange { value } => write!(
                f,
                "tuning.mrft_delay_secs must be between 0 and {MAX_TUNING_MRFT_DELAY_SECS}, got {value}"
            ),
            Self::PollIntervalTooSmall { value } => {
                write!(f, "tuning.poll_interval_ms must be at least 1, got {value}")
            }
            Self::TimeoutTooSmall { value } => {
                write!(f, "tuning.timeout_secs must be at least 1, got {value}")
            }
            Self::OpTimeoutTooSmall { value } => {
                write!(f, "tuning.op_timeout_secs must be at least 1, got {value}")
            }
            Self::RestoreTimeoutTooSmall { value, minimum } => write!(
                f,
                "tuning.restore_timeout_secs must be at least {minimum}, got {value}"
            ),
        }
    }
}

impl std::error::Error for TuningConfigError {}

/// Resolve optional `[tuning]` values against the built-in defaults.
pub fn resolve_tuning_config(config: &TuningConfig) -> EffectiveTuningConfig {
    EffectiveTuningConfig {
        mrft_delay_secs: config
            .mrft_delay_secs
            .unwrap_or(DEFAULT_TUNING_MRFT_DELAY_SECS),
        poll_interval_ms: config
            .poll_interval_ms
            .unwrap_or(DEFAULT_TUNING_POLL_INTERVAL_MS),
        timeout_secs: config.timeout_secs.unwrap_or(DEFAULT_TUNING_TIMEOUT_SECS),
        op_timeout_secs: config
            .op_timeout_secs
            .unwrap_or(DEFAULT_TUNING_OP_TIMEOUT_SECS),
        restore_timeout_secs: config
            .restore_timeout_secs
            .unwrap_or(DEFAULT_TUNING_RESTORE_TIMEOUT_SECS),
    }
}

/// Report whether each effective tuning value came from TOML or a built-in default.
pub fn tuning_config_sources(config: &TuningConfig) -> TuningConfigSources {
    fn source<T>(value: Option<T>) -> TuningConfigSource {
        if value.is_some() {
            TuningConfigSource::Toml
        } else {
            TuningConfigSource::BuiltInDefault
        }
    }

    TuningConfigSources {
        mrft_delay_secs: source(config.mrft_delay_secs),
        poll_interval_ms: source(config.poll_interval_ms),
        timeout_secs: source(config.timeout_secs),
        op_timeout_secs: source(config.op_timeout_secs),
        restore_timeout_secs: source(config.restore_timeout_secs),
    }
}

/// Validate concrete tuning timing values.
///
/// `require_opc_restore_minimum` raises the restoration minimum from one second to
/// [`MIN_OPC_RESTORE_TIMEOUT_SECS`], matching the live OPC DA actuation-confirmation window.
pub fn validate_tuning_config(
    config: &EffectiveTuningConfig,
    require_opc_restore_minimum: bool,
) -> Result<(), TuningConfigError> {
    if config.mrft_delay_secs > MAX_TUNING_MRFT_DELAY_SECS {
        return Err(TuningConfigError::MrftDelayOutOfRange {
            value: config.mrft_delay_secs,
        });
    }
    if config.poll_interval_ms == 0 {
        return Err(TuningConfigError::PollIntervalTooSmall {
            value: config.poll_interval_ms,
        });
    }
    if config.timeout_secs == 0 {
        return Err(TuningConfigError::TimeoutTooSmall {
            value: config.timeout_secs,
        });
    }
    if config.op_timeout_secs == 0 {
        return Err(TuningConfigError::OpTimeoutTooSmall {
            value: config.op_timeout_secs,
        });
    }
    let restore_minimum = if require_opc_restore_minimum {
        MIN_OPC_RESTORE_TIMEOUT_SECS
    } else {
        1
    };
    if config.restore_timeout_secs < restore_minimum {
        return Err(TuningConfigError::RestoreTimeoutTooSmall {
            value: config.restore_timeout_secs,
            minimum: restore_minimum,
        });
    }
    Ok(())
}

/// Resolve and validate a raw `[tuning]` table in one step.
pub fn resolve_and_validate_tuning_config(
    config: &TuningConfig,
    require_opc_restore_minimum: bool,
) -> Result<EffectiveTuningConfig, TuningConfigError> {
    let effective = resolve_tuning_config(config);
    validate_tuning_config(&effective, require_opc_restore_minimum)?;
    Ok(effective)
}

/// bhtune's configuration, loaded from an optional TOML file. Every field is optional; a
/// value missing from the file (or the file itself missing) falls back to the env var / CLI
/// flag / built-in default resolution in the `resolve_*` functions below.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct BhtuneConfig {
    /// Runtime server mode. The environment variable `BHTUNE_SERVER_MODE` overrides this.
    #[serde(default)]
    pub server_mode: Option<ServerMode>,
    /// Safety limits for the simulator-only public demo mode.
    #[serde(default)]
    pub demo: DemoPolicyConfig,
    /// Overrides the default SQLite database path (see [`default_db_path_from`]).
    pub db: Option<PathBuf>,
    /// Overrides [`DEFAULT_BRIDGE_HOST`] for every `tune --driver opcda` and `opc`
    /// subcommand invocation that doesn't pass `--bridge-host` explicitly.
    pub bridge_host: Option<String>,
    /// Default OPC DA server ProgID, used when `--server` is omitted. Unlike the other
    /// fields there is no built-in default -- if this is unset and `--server` is omitted,
    /// the command errors (see [`resolve_server`]).
    pub server: Option<String>,
    /// Overrides the default user-supplied DCS/PLC template catalog path (see
    /// [`templates_path_from`]). A file here is loaded on every startup in addition to the
    /// embedded built-in catalog (see `crate::db::open` and [`load_user_templates`]),
    /// attributed `TemplateOrigin::Catalog`.
    pub templates: Option<PathBuf>,
    /// Overrides [`DEFAULT_BIND_ADDR`] -- the `host:port` `bhtune-server` listens on. Only
    /// meaningful to the server binary; see [`resolve_bind_addr`].
    pub bind: Option<String>,
    /// Exact browser origin allowed for state-changing HTTP requests. `BHTUNE_ORIGIN`
    /// overrides this value. Demo mode requires HTTPS, except for explicit loopback HTTP
    /// origins used by local tests and development.
    #[serde(default)]
    pub origin: Option<String>,
    /// IP address or matching-family CIDR of a reverse proxy trusted to supply the
    /// single-address `X-BHTune-Client-IP` header for Demo quota accounting.
    #[serde(default)]
    pub trusted_proxy: Option<String>,
    /// Age-based history retention (`history-retention`): tune runs with `started_at` older
    /// than this many days are deleted automatically on every startup (both binaries, via
    /// `crate::db::open`) and, for `bhtune-server`, again on a periodic timer while it keeps
    /// running -- see `crate::retention`. A present value must be at least 1. `None` (the
    /// default) means retain forever: there is no built-in number of days, since at this
    /// project's data volumes (see AGENTS.md's History explorer notes) an unexpected
    /// auto-delete of someone's baseline tune is a worse failure mode than an ever-growing
    /// database file. See [`resolve_retention_days`].
    #[serde(default, deserialize_with = "deserialize_retention_days")]
    #[cfg_attr(feature = "schemars", schemars(range(min = 1)))]
    pub retention_days: Option<u32>,
    /// Default OPC sample-quality policy for the server config page: `true` accepts
    /// `Uncertain` quality, while `false` rejects it. A missing key is treated as `true`
    /// when the config file is parsed, matching the configuration-page default rather than
    /// `bool`'s ordinary `false`.
    #[serde(default = "default_allow_uncertain_quality")]
    pub allow_uncertain_quality: bool,
    /// Global tune timing defaults. Missing keys remain `None` and resolve through
    /// [`resolve_tuning_config`] only when a tune is prepared.
    #[serde(default)]
    pub tuning: TuningConfig,
    /// `[log]` sub-table: level/directory/format/rotation for `crate::logging`'s tracing
    /// setup, mirroring `opcda-bridge-gateway`'s own `log.*` config conventions.
    #[serde(default)]
    pub log: LogConfig,
}

/// Logging configuration keys (a `[log]` table in `bhtune.toml`), consumed by
/// `crate::logging::resolve_log_settings`. Every field is optional and falls back through
/// the same `CLI flag > env var > config file > default` precedence as the rest of
/// [`BhtuneConfig`].
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct LogConfig {
    pub level: Option<String>,
    pub dir: Option<String>,
    pub format: Option<String>,
    pub rotation: Option<String>,
}

/// The default configuration-page quality policy: absent `allow_uncertain_quality` resolves
/// to `true` rather than `bool`'s usual `false`.
pub const fn default_allow_uncertain_quality() -> bool {
    true
}

fn deserialize_retention_days<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let days = Option::<u32>::deserialize(deserializer)?;
    match days {
        Some(0) => Err(serde::de::Error::custom(
            "retention_days must be at least 1 or omitted",
        )),
        other => Ok(other),
    }
}

impl Default for BhtuneConfig {
    fn default() -> Self {
        Self {
            server_mode: None,
            demo: DemoPolicyConfig::default(),
            db: None,
            bridge_host: None,
            server: None,
            templates: None,
            bind: None,
            origin: None,
            trusted_proxy: None,
            retention_days: None,
            allow_uncertain_quality: default_allow_uncertain_quality(),
            tuning: TuningConfig::default(),
            log: LogConfig::default(),
        }
    }
}

/// Path-resolution result for the TOML config store: either an explicit `--config` path or
/// the auto-discovered default, plus whether a missing file is acceptable at that tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPathResolution {
    pub path: Option<PathBuf>,
    pub missing_is_allowed: bool,
}

/// A path-aware TOML config snapshot suitable for server-side read/modify/write flows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedConfigStore {
    pub path: Option<PathBuf>,
    pub missing_is_allowed: bool,
    pub original_raw: Option<String>,
    pub config: BhtuneConfig,
    pub revision: String,
    /// Raw file value, if the key was present. This distinguishes an explicit `true` from
    /// the defaulted value when reporting configuration provenance.
    pub toml_allow_uncertain_quality: Option<bool>,
    /// Raw optional values from the TOML `[tuning]` table.
    pub toml_tuning: TuningConfig,
    /// Per-field TOML/default provenance for [`Self::toml_tuning`].
    pub tuning_sources: TuningConfigSources,
}

/// Config-page-owned settings that can be patched in place while preserving every unrelated
/// key and comment in the source TOML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPolicyUpdate {
    pub allow_uncertain_quality: bool,
    pub retention_days: Option<u32>,
    pub mrft_delay_secs: Option<u32>,
    pub poll_interval_ms: Option<u64>,
    pub timeout_secs: Option<u64>,
    pub op_timeout_secs: Option<u64>,
    pub restore_timeout_secs: Option<u64>,
}

impl ConfigPolicyUpdate {
    pub fn tuning(&self) -> TuningConfig {
        TuningConfig {
            mrft_delay_secs: self.mrft_delay_secs,
            poll_interval_ms: self.poll_interval_ms,
            timeout_secs: self.timeout_secs,
            op_timeout_secs: self.op_timeout_secs,
            restore_timeout_secs: self.restore_timeout_secs,
        }
    }
}

impl Default for ConfigPolicyUpdate {
    fn default() -> Self {
        Self {
            allow_uncertain_quality: default_allow_uncertain_quality(),
            retention_days: None,
            mrft_delay_secs: None,
            poll_interval_ms: None,
            timeout_secs: None,
            op_timeout_secs: None,
            restore_timeout_secs: None,
        }
    }
}

/// Result of safely saving a patched TOML config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSaveResult {
    pub backup_path: Option<PathBuf>,
    pub state: LoadedConfigStore,
}

/// Typed errors for the path-aware TOML config store used by the server config page.
#[derive(Debug)]
pub enum ConfigStoreError {
    PathNotResolved,
    Missing {
        path: PathBuf,
    },
    Unreadable {
        path: PathBuf,
        source: io::Error,
    },
    Malformed {
        path: Option<PathBuf>,
        source: String,
    },
    Conflict {
        path: Option<PathBuf>,
        message: String,
    },
    Write {
        path: PathBuf,
        action: &'static str,
        source: io::Error,
    },
}

impl std::fmt::Display for ConfigStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PathNotResolved => write!(
                f,
                "no config path could be resolved from --config / XDG_CONFIG_HOME / HOME / APPDATA"
            ),
            Self::Missing { path } => write!(f, "config file not found: {}", path.display()),
            Self::Unreadable { path, source } => {
                write!(f, "failed to read config file {}: {source}", path.display())
            }
            Self::Malformed {
                path: Some(path),
                source,
            } => write!(
                f,
                "failed to parse config file {}: {source}",
                path.display()
            ),
            Self::Malformed { path: None, source } => {
                write!(f, "failed to parse config contents: {source}")
            }
            Self::Conflict {
                path: Some(path),
                message,
            } => write!(f, "config store conflict for {}: {message}", path.display()),
            Self::Conflict {
                path: None,
                message,
            } => write!(f, "config store conflict: {message}"),
            Self::Write {
                path,
                action,
                source,
            } => write!(f, "failed to {action} {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for ConfigStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unreadable { source, .. } | Self::Write { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Derive bhtune's config file location from raw environment values rather than reading
/// `std::env` directly -- keeps discovery fully unit-testable across every permutation
/// without mutating real process environment variables.
///
/// - Windows (`is_windows = true`): `%APPDATA%\bhtune\bhtune.toml`.
/// - Elsewhere: `$XDG_CONFIG_HOME/bhtune/bhtune.toml`, falling back to
///   `$HOME/.config/bhtune/bhtune.toml`.
pub fn config_path_from(
    xdg_config_home: Option<&str>,
    home: Option<&str>,
    appdata: Option<&str>,
    is_windows: bool,
) -> Option<PathBuf> {
    if is_windows {
        return appdata.map(|dir| Path::new(dir).join("bhtune").join("bhtune.toml"));
    }
    if let Some(dir) = xdg_config_home {
        return Some(Path::new(dir).join("bhtune").join("bhtune.toml"));
    }
    home.map(|dir| {
        Path::new(dir)
            .join(".config")
            .join("bhtune")
            .join("bhtune.toml")
    })
}

/// Resolve which path the TOML config store should use: an explicit `--config` path if one
/// was provided, otherwise the platform-default `bhtune.toml` location from
/// [`config_path_from`]. Explicit paths must already exist; an auto-discovered missing file
/// is acceptable and can be created on first save.
pub fn resolve_config_store_path(
    explicit_path: Option<&Path>,
    xdg_config_home: Option<&str>,
    home: Option<&str>,
    appdata: Option<&str>,
    is_windows: bool,
) -> ConfigPathResolution {
    match explicit_path {
        Some(path) => ConfigPathResolution {
            path: Some(path.to_path_buf()),
            missing_is_allowed: false,
        },
        None => ConfigPathResolution {
            path: config_path_from(xdg_config_home, home, appdata, is_windows),
            missing_is_allowed: true,
        },
    }
}

const FNV1A_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV1A_PRIME: u64 = 0x100000001b3;

fn stable_revision_hash(bytes: &[u8]) -> u64 {
    let mut hash = FNV1A_OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV1A_PRIME);
    }
    hash
}

fn revision_token_for_raw(raw: Option<&str>) -> String {
    match raw {
        Some(raw) => format!(
            "present:v1:{}:{:016x}",
            raw.len(),
            stable_revision_hash(raw.as_bytes())
        ),
        None => "absent:v1".to_string(),
    }
}

fn config_malformed(path: Option<&Path>, error: impl std::fmt::Display) -> ConfigStoreError {
    ConfigStoreError::Malformed {
        path: path.map(Path::to_path_buf),
        source: error.to_string(),
    }
}

fn load_config_store_from_resolution(
    resolution: ConfigPathResolution,
) -> Result<LoadedConfigStore, ConfigStoreError> {
    match resolution.path {
        Some(path) => match fs::read(&path) {
            Ok(bytes) => {
                let raw = String::from_utf8(bytes).map_err(|e| {
                    config_malformed(Some(&path), format!("config file is not valid UTF-8: {e}"))
                })?;
                let config =
                    parse_config_contents(&raw).map_err(|e| config_malformed(Some(&path), e))?;
                let toml_allow_uncertain_quality = raw
                    .parse::<toml_edit::DocumentMut>()
                    .ok()
                    .and_then(|document| {
                        document
                            .get("allow_uncertain_quality")
                            .and_then(|item| item.as_value())
                            .and_then(|value| value.as_bool())
                    });
                let toml_tuning = config.tuning;
                let tuning_sources = tuning_config_sources(&toml_tuning);
                let revision = revision_token_for_raw(Some(&raw));
                Ok(LoadedConfigStore {
                    path: Some(path),
                    missing_is_allowed: resolution.missing_is_allowed,
                    original_raw: Some(raw),
                    config,
                    revision,
                    toml_allow_uncertain_quality,
                    toml_tuning,
                    tuning_sources,
                })
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound && resolution.missing_is_allowed => {
                let revision = revision_token_for_raw(None);
                Ok(LoadedConfigStore {
                    path: Some(path),
                    missing_is_allowed: true,
                    original_raw: None,
                    config: BhtuneConfig::default(),
                    revision,
                    toml_allow_uncertain_quality: None,
                    toml_tuning: TuningConfig::default(),
                    tuning_sources: tuning_config_sources(&TuningConfig::default()),
                })
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(ConfigStoreError::Missing { path })
            }
            Err(e) => Err(ConfigStoreError::Unreadable { path, source: e }),
        },
        None => Ok(LoadedConfigStore {
            path: None,
            missing_is_allowed: true,
            original_raw: None,
            config: BhtuneConfig::default(),
            revision: revision_token_for_raw(None),
            toml_allow_uncertain_quality: None,
            toml_tuning: TuningConfig::default(),
            tuning_sources: tuning_config_sources(&TuningConfig::default()),
        }),
    }
}

/// Derive bhtune's default *user template catalog* location the same way [`config_path_from`]
/// derives `bhtune.toml`'s -- deliberately the same directory, since both are per-user
/// settings a site admin edits by hand, not persistent application data (contrast
/// [`default_db_path_from`]/[`default_log_dir_from`], which live under the platform data
/// directory instead). See [`load_user_templates`] for how this default fits into the full
/// `template-user-catalog` precedence chain.
///
/// - Windows (`is_windows = true`): `%APPDATA%\bhtune\templates.toml`.
/// - Elsewhere: `$XDG_CONFIG_HOME/bhtune/templates.toml`, falling back to
///   `$HOME/.config/bhtune/templates.toml`.
pub fn templates_path_from(
    xdg_config_home: Option<&str>,
    home: Option<&str>,
    appdata: Option<&str>,
    is_windows: bool,
) -> Option<PathBuf> {
    if is_windows {
        return appdata.map(|dir| Path::new(dir).join("bhtune").join("templates.toml"));
    }
    if let Some(dir) = xdg_config_home {
        return Some(Path::new(dir).join("bhtune").join("templates.toml"));
    }
    home.map(|dir| {
        Path::new(dir)
            .join(".config")
            .join("bhtune")
            .join("templates.toml")
    })
}

/// Derive bhtune's default *database* location the same way config files are discovered, but
/// under the platform's data directory rather than its config directory -- a database is
/// persistent user data, not settings, per the XDG base directory specification.
///
/// - Windows (`is_windows = true`): `%APPDATA%\bhtune\bhtune.db`.
/// - Elsewhere: `$XDG_DATA_HOME/bhtune/bhtune.db`, falling back to
///   `$HOME/.local/share/bhtune/bhtune.db`.
/// - A database location must always resolve to *something* usable (unlike the config
///   file, whose absence is fine) -- if none of the above are available, this falls back
///   further to `bhtune.db` in the current directory, matching the CLI's original
///   hardcoded placeholder default.
pub fn default_db_path_from(
    xdg_data_home: Option<&str>,
    home: Option<&str>,
    appdata: Option<&str>,
    is_windows: bool,
) -> PathBuf {
    if is_windows {
        return appdata
            .map(|dir| Path::new(dir).join("bhtune").join("bhtune.db"))
            .unwrap_or_else(|| PathBuf::from("bhtune.db"));
    }
    if let Some(dir) = xdg_data_home {
        return Path::new(dir).join("bhtune").join("bhtune.db");
    }
    home.map(|dir| {
        Path::new(dir)
            .join(".local")
            .join("share")
            .join("bhtune")
            .join("bhtune.db")
    })
    .unwrap_or_else(|| PathBuf::from("bhtune.db"))
}

/// Derive bhtune's default *log directory* the same way the database path is derived (see
/// [`default_db_path_from`]) -- under the platform data directory, not next to the compiled
/// binary. Unlike `opcda-bridge-gateway`'s equivalent (`log_dir_from_exe`), a `cargo
/// install`ed binary's own directory (e.g. `~/.cargo/bin/`) isn't a sensible place to write
/// logs, and bhtune already has this exact precedence machinery for the database, so the log
/// directory reuses it rather than inventing a second convention.
///
/// - Windows (`is_windows = true`): `%APPDATA%\bhtune\logs`.
/// - Elsewhere: `$XDG_DATA_HOME/bhtune/logs`, falling back to `$HOME/.local/share/bhtune/logs`.
/// - Falls back further to `logs` in the current directory if none of the above are
///   available, matching [`default_db_path_from`]'s own final fallback.
pub fn default_log_dir_from(
    xdg_data_home: Option<&str>,
    home: Option<&str>,
    appdata: Option<&str>,
    is_windows: bool,
) -> PathBuf {
    if is_windows {
        return appdata
            .map(|dir| Path::new(dir).join("bhtune").join("logs"))
            .unwrap_or_else(|| PathBuf::from("logs"));
    }
    if let Some(dir) = xdg_data_home {
        return Path::new(dir).join("bhtune").join("logs");
    }
    home.map(|dir| {
        Path::new(dir)
            .join(".local")
            .join("share")
            .join("bhtune")
            .join("logs")
    })
    .unwrap_or_else(|| PathBuf::from("logs"))
}

/// Load a bhtune config from `path`.
///
/// A missing file resolves to `Ok(BhtuneConfig::default())` when `missing_is_error` is
/// false (the auto-discovered path may legitimately not exist yet); with an explicit
/// `--config` path a missing file is a hard error instead. A file that exists but fails to
/// parse as TOML is always a hard error -- a config typo should never be silently ignored.
pub fn load_config_file(path: &Path, missing_is_error: bool) -> anyhow::Result<BhtuneConfig> {
    load_config_store_from_resolution(ConfigPathResolution {
        path: Some(path.to_path_buf()),
        missing_is_allowed: !missing_is_error,
    })
    .map(|store| store.config)
    .map_err(|e| anyhow::anyhow!(e.to_string()))
}

/// Parses the in-memory TOML representation of a config file.
///
/// Keeping the parser separate from filesystem discovery gives property tests and fuzz
/// targets a narrow, side-effect-free boundary to exercise.
pub fn parse_config_contents(contents: &str) -> anyhow::Result<BhtuneConfig> {
    toml::from_str(contents).map_err(Into::into)
}

fn patch_config_contents<F>(raw: Option<&str>, mutator: F) -> Result<(String, BhtuneConfig), String>
where
    F: FnOnce(&mut toml_edit::DocumentMut),
{
    let mut document = raw
        .unwrap_or_default()
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| e.to_string())?;
    mutator(&mut document);
    let patched = document.to_string();
    let parsed = parse_config_contents(&patched).map_err(|e| e.to_string())?;
    Ok((patched, parsed))
}

/// Patch only `allow_uncertain_quality` while preserving every unrelated key, comment, and
/// formatting detail the source document already had.
pub fn patch_allow_uncertain_quality(
    raw: Option<&str>,
    allow_uncertain_quality: bool,
) -> Result<String, ConfigStoreError> {
    patch_config_contents(raw, |document| {
        document["allow_uncertain_quality"] = toml_edit::value(allow_uncertain_quality);
    })
    .map_err(|source| config_malformed(None, source))
    .map(|(patched, _)| patched)
}

/// Patch only `retention_days` while preserving every unrelated key, comment, and formatting
/// detail the source document already had. `None` removes the key entirely.
pub fn patch_retention_days(
    raw: Option<&str>,
    retention_days: Option<u32>,
) -> Result<String, ConfigStoreError> {
    patch_config_contents(raw, |document| match retention_days {
        Some(days) => {
            document["retention_days"] = toml_edit::value(i64::from(days));
        }
        None => {
            document.as_table_mut().remove("retention_days");
        }
    })
    .map_err(|source| config_malformed(None, source))
    .map(|(patched, _)| patched)
}

fn patch_optional_tuning_value<T>(
    document: &mut toml_edit::DocumentMut,
    key: &str,
    value: Option<T>,
) where
    T: Into<toml_edit::Value>,
{
    match value {
        Some(value) => {
            if document.get("tuning").is_none() {
                document["tuning"] = toml_edit::table();
            }
            document["tuning"][key] = toml_edit::value(value);
        }
        None => {
            if let Some(table) = document
                .get_mut("tuning")
                .and_then(toml_edit::Item::as_table_like_mut)
            {
                table.remove(key);
            }
        }
    }
}

fn optional_u64_to_toml_integer(
    field: &'static str,
    value: Option<u64>,
) -> Result<Option<i64>, String> {
    value
        .map(|value| {
            i64::try_from(value)
                .map_err(|_| format!("tuning.{field} is too large to store as a TOML integer"))
        })
        .transpose()
}

/// Patch all five `[tuning]` values while preserving unrelated keys, comments, and formatting.
/// A `None` value removes only that key.
pub fn patch_tuning_config(
    raw: Option<&str>,
    tuning: &TuningConfig,
) -> Result<String, ConfigStoreError> {
    resolve_and_validate_tuning_config(tuning, false)
        .map_err(|source| config_malformed(None, source))?;
    let poll_interval_ms =
        optional_u64_to_toml_integer("poll_interval_ms", tuning.poll_interval_ms)
            .map_err(|source| config_malformed(None, source))?;
    let timeout_secs = optional_u64_to_toml_integer("timeout_secs", tuning.timeout_secs)
        .map_err(|source| config_malformed(None, source))?;
    let op_timeout_secs = optional_u64_to_toml_integer("op_timeout_secs", tuning.op_timeout_secs)
        .map_err(|source| config_malformed(None, source))?;
    let restore_timeout_secs =
        optional_u64_to_toml_integer("restore_timeout_secs", tuning.restore_timeout_secs)
            .map_err(|source| config_malformed(None, source))?;
    let (patched, parsed) = patch_config_contents(raw, |document| {
        patch_optional_tuning_value(
            document,
            "mrft_delay_secs",
            tuning.mrft_delay_secs.map(i64::from),
        );
        patch_optional_tuning_value(document, "poll_interval_ms", poll_interval_ms);
        patch_optional_tuning_value(document, "timeout_secs", timeout_secs);
        patch_optional_tuning_value(document, "op_timeout_secs", op_timeout_secs);
        patch_optional_tuning_value(document, "restore_timeout_secs", restore_timeout_secs);
    })
    .map_err(|source| config_malformed(None, source))?;
    resolve_and_validate_tuning_config(&parsed.tuning, false)
        .map_err(|source| config_malformed(None, source))?;
    Ok(patched)
}

fn patch_config_policy(
    raw: Option<&str>,
    update: &ConfigPolicyUpdate,
) -> Result<(String, BhtuneConfig), String> {
    let tuning = update.tuning();
    resolve_and_validate_tuning_config(&tuning, false).map_err(|e| e.to_string())?;
    let poll_interval_ms =
        optional_u64_to_toml_integer("poll_interval_ms", tuning.poll_interval_ms)?;
    let timeout_secs = optional_u64_to_toml_integer("timeout_secs", tuning.timeout_secs)?;
    let op_timeout_secs = optional_u64_to_toml_integer("op_timeout_secs", tuning.op_timeout_secs)?;
    let restore_timeout_secs =
        optional_u64_to_toml_integer("restore_timeout_secs", tuning.restore_timeout_secs)?;
    let result = patch_config_contents(raw, |document| {
        document["allow_uncertain_quality"] = toml_edit::value(update.allow_uncertain_quality);
        match update.retention_days {
            Some(days) => {
                document["retention_days"] = toml_edit::value(i64::from(days));
            }
            None => {
                document.as_table_mut().remove("retention_days");
            }
        }
        patch_optional_tuning_value(
            document,
            "mrft_delay_secs",
            tuning.mrft_delay_secs.map(i64::from),
        );
        patch_optional_tuning_value(document, "poll_interval_ms", poll_interval_ms);
        patch_optional_tuning_value(document, "timeout_secs", timeout_secs);
        patch_optional_tuning_value(document, "op_timeout_secs", op_timeout_secs);
        patch_optional_tuning_value(document, "restore_timeout_secs", restore_timeout_secs);
    })?;
    resolve_and_validate_tuning_config(&result.1.tuning, false).map_err(|e| e.to_string())?;
    Ok(result)
}

/// Load the path-aware TOML config store using real environment-based auto-discovery.
pub fn load_config_store(
    explicit_path: Option<&Path>,
) -> Result<LoadedConfigStore, ConfigStoreError> {
    load_config_store_from(
        explicit_path,
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        std::env::var("APPDATA").ok().as_deref(),
        cfg!(target_os = "windows"),
    )
}

/// Load the path-aware TOML config store using injected path-discovery inputs so every
/// resolution branch stays unit-testable without mutating process-global environment
/// variables.
pub fn load_config_store_from(
    explicit_path: Option<&Path>,
    xdg_config_home: Option<&str>,
    home: Option<&str>,
    appdata: Option<&str>,
    is_windows: bool,
) -> Result<LoadedConfigStore, ConfigStoreError> {
    load_config_store_from_resolution(resolve_config_store_path(
        explicit_path,
        xdg_config_home,
        home,
        appdata,
        is_windows,
    ))
}

fn ensure_parent_dir(path: &Path) -> Result<(), ConfigStoreError> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| {
            fs::create_dir_all(parent).map_err(|e| ConfigStoreError::Write {
                path: path.to_path_buf(),
                action: "create config directory",
                source: e,
            })
        })
        .transpose()
        .map(|_| ())
}

fn unique_suffix() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{}-{}-{:09}",
        std::process::id(),
        timestamp.as_secs(),
        timestamp.subsec_nanos()
    )
}

fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut file_name = path
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from("bhtune.toml"));
    file_name.push(format!(".{suffix}-{}", unique_suffix()));
    path.with_file_name(file_name)
}

fn backup_path_for(path: &Path) -> PathBuf {
    let mut file_name = path
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from("bhtune.toml"));
    file_name.push(format!(".backup-{}.bak", unique_suffix()));
    path.with_file_name(file_name)
}

fn create_temp_file(path: &Path) -> Result<(PathBuf, fs::File), ConfigStoreError> {
    create_temp_file_with(path, || sibling_with_suffix(path, "tmp"))
}

fn create_temp_file_with<F>(
    path: &Path,
    mut next_path: F,
) -> Result<(PathBuf, fs::File), ConfigStoreError>
where
    F: FnMut() -> PathBuf,
{
    for _ in 0..16 {
        let temp_path = next_path();
        match fs::OpenOptions::new()
            .create_new(true)
            .truncate(true)
            .write(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(ConfigStoreError::Write {
                    path: path.to_path_buf(),
                    action: "create temporary config file",
                    source: e,
                });
            }
        }
    }

    Err(ConfigStoreError::Write {
        path: path.to_path_buf(),
        action: "create temporary config file",
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "exhausted unique temp-file names",
        ),
    })
}

trait SyncConfigFile {
    fn sync_config(&self) -> io::Result<()>;
}

impl SyncConfigFile for fs::File {
    fn sync_config(&self) -> io::Result<()> {
        self.sync_all()
    }
}

fn write_and_flush_temp_file<T>(
    path: &Path,
    temp_path: &Path,
    bytes: &[u8],
    temp_file: &mut T,
) -> Result<(), ConfigStoreError>
where
    T: Write + SyncConfigFile,
{
    if let Err(e) = temp_file.write_all(bytes) {
        let _ = fs::remove_file(temp_path);
        return Err(ConfigStoreError::Write {
            path: path.to_path_buf(),
            action: "write temporary config file",
            source: e,
        });
    }
    if let Err(e) = temp_file.sync_config() {
        let _ = fs::remove_file(temp_path);
        return Err(ConfigStoreError::Write {
            path: path.to_path_buf(),
            action: "flush temporary config file",
            source: e,
        });
    }
    Ok(())
}

#[cfg(unix)]
fn sync_parent_dir(path: &Path) {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        && let Ok(dir) = fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) {}

fn write_config_file_atomically(
    path: &Path,
    bytes: &[u8],
    create_parent_dir: bool,
) -> Result<Option<PathBuf>, ConfigStoreError> {
    write_config_file_atomically_with(
        path,
        bytes,
        create_parent_dir,
        |source, destination| fs::copy(source, destination),
        |source, destination| fs::rename(source, destination),
    )
}

fn write_config_file_atomically_with<Copy, Rename>(
    path: &Path,
    bytes: &[u8],
    create_parent_dir: bool,
    copy_backup: Copy,
    replace: Rename,
) -> Result<Option<PathBuf>, ConfigStoreError>
where
    Copy: Fn(&Path, &Path) -> io::Result<u64>,
    Rename: Fn(&Path, &Path) -> io::Result<()>,
{
    if create_parent_dir {
        ensure_parent_dir(path)?;
    }

    let (temp_path, mut temp_file) = create_temp_file(path)?;
    write_and_flush_temp_file(path, &temp_path, bytes, &mut temp_file)?;
    drop(temp_file);

    let backup_path = if path.exists() {
        let backup_path = backup_path_for(path);
        if let Err(e) = copy_backup(path, &backup_path) {
            let _ = fs::remove_file(&temp_path);
            return Err(ConfigStoreError::Write {
                path: path.to_path_buf(),
                action: "create config backup",
                source: e,
            });
        }
        Some(backup_path)
    } else {
        None
    };

    let replace_result = replace(&temp_path, path);

    if let Err(e) = replace_result {
        let _ = fs::remove_file(&temp_path);
        return Err(ConfigStoreError::Write {
            path: path.to_path_buf(),
            action: "replace config file",
            source: e,
        });
    }

    sync_parent_dir(path);
    Ok(backup_path)
}

/// Safely save the two config-page-managed settings with optimistic-concurrency checks.
///
/// The caller must provide the revision token it last loaded. The save is rejected when that
/// token is stale *or* the on-disk bytes no longer match the bytes this store last loaded or
/// wrote, preventing blind overwrites of external edits.
pub fn save_config_store(
    state: &LoadedConfigStore,
    expected_revision: &str,
    update: &ConfigPolicyUpdate,
) -> Result<ConfigSaveResult, ConfigStoreError> {
    if state.revision != expected_revision {
        return Err(ConfigStoreError::Conflict {
            path: state.path.clone(),
            message: format!(
                "stale config revision token: expected {expected_revision}, latest {}",
                state.revision
            ),
        });
    }

    let path = state
        .path
        .clone()
        .ok_or(ConfigStoreError::PathNotResolved)?;

    let current_bytes = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(ConfigStoreError::Unreadable {
                path: path.clone(),
                source: e,
            });
        }
    };
    let loaded_bytes = state.original_raw.as_ref().map(String::as_bytes);
    let disk_matches_loaded = match (loaded_bytes, current_bytes.as_deref()) {
        (None, None) => true,
        (Some(loaded), Some(current)) => loaded == current,
        _ => false,
    };
    if !disk_matches_loaded {
        return Err(ConfigStoreError::Conflict {
            path: Some(path.clone()),
            message: "config file changed on disk since it was loaded".to_string(),
        });
    }

    if state.original_raw.is_none() && !state.missing_is_allowed {
        return Err(ConfigStoreError::Missing { path });
    }

    let (patched_raw, config) = patch_config_policy(state.original_raw.as_deref(), update)
        .map_err(|source| config_malformed(Some(&path), source))?;
    let backup_path =
        write_config_file_atomically(&path, patched_raw.as_bytes(), state.original_raw.is_none())?;
    let revision = revision_token_for_raw(Some(&patched_raw));
    let toml_allow_uncertain_quality = Some(update.allow_uncertain_quality);
    let toml_tuning = update.tuning();
    let tuning_sources = tuning_config_sources(&toml_tuning);

    Ok(ConfigSaveResult {
        backup_path,
        state: LoadedConfigStore {
            path: Some(path),
            missing_is_allowed: state.missing_is_allowed,
            original_raw: Some(patched_raw),
            config,
            revision,
            toml_allow_uncertain_quality,
            toml_tuning,
            tuning_sources,
        },
    })
}

/// Load the config from an auto-discovered path, falling back to defaults when no path
/// could be discovered at all (e.g. neither `XDG_CONFIG_HOME` nor `HOME` is set on a
/// non-Windows host). Split out from [`load_config`] so the no-path-discovered branch is
/// directly unit-testable with a literal `None`, without mutating real process-global
/// environment variables in a parallel test binary.
fn load_discovered_config(path: Option<PathBuf>) -> anyhow::Result<BhtuneConfig> {
    load_config_store_from_resolution(ConfigPathResolution {
        path,
        missing_is_allowed: true,
    })
    .map(|store| store.config)
    .map_err(|e| anyhow::anyhow!(e.to_string()))
}

/// Resolve and load the bhtune config: an explicit `--config` path if given, otherwise the
/// platform's auto-discovered path (silently falls back to defaults if none of the relevant
/// environment variables are set, or if the discovered file doesn't exist).
pub fn load_config(explicit_path: Option<&Path>) -> anyhow::Result<BhtuneConfig> {
    match explicit_path {
        Some(path) => load_config_file(path, true),
        None => {
            let path = config_path_from(
                std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
                std::env::var("HOME").ok().as_deref(),
                std::env::var("APPDATA").ok().as_deref(),
                cfg!(target_os = "windows"),
            );
            load_discovered_config(path)
        }
    }
}

/// Resolve and load the user-supplied DCS/PLC template catalog (`template-user-catalog`):
/// `--templates` / `BHTUNE_TEMPLATES` (already folded into `cli_templates` by clap's `env`
/// attribute) / the config file's `templates` key, or else the platform's auto-discovered
/// `templates.toml` next to `bhtune.toml` (see [`templates_path_from`]) -- `CLI flag > env
/// var > config file > platform default`, the same precedence chain as every other bhtune
/// setting.
///
/// Returns `Ok(None)` when no catalog applies at all: nothing was requested explicitly and
/// no file exists at the auto-discovered default path -- the common case, since most
/// installs never create this file. A path named *explicitly* (by any of the first three
/// tiers) that doesn't exist is a hard error, exactly mirroring `bhtune.toml` itself
/// (`load_config_file`'s `missing_is_error`); a file that exists (whichever tier it came
/// from) but fails to parse as TOML or fails `DcsTemplate::validate()` is always a hard
/// error naming the file and the problem -- a malformed catalog should never be silently
/// ignored. Parsing and validating are both handled by
/// [`bhtune_core::template::parse_catalog`], so this function does no TOML-shape or
/// cross-field checking of its own.
pub fn load_user_templates(
    cli_templates: Option<PathBuf>,
    config: &BhtuneConfig,
    xdg_config_home: Option<&str>,
    home: Option<&str>,
    appdata: Option<&str>,
    is_windows: bool,
) -> anyhow::Result<Option<Vec<bhtune_core::DcsTemplate>>> {
    let (path, missing_is_error) = match cli_templates.or_else(|| config.templates.clone()) {
        Some(explicit) => (Some(explicit), true),
        None => (
            templates_path_from(xdg_config_home, home, appdata, is_windows),
            false,
        ),
    };
    let Some(path) = path else {
        return Ok(None);
    };

    match std::fs::read_to_string(&path) {
        Ok(contents) => bhtune_core::template::parse_catalog(&contents)
            .map(Some)
            .map_err(|e| anyhow::anyhow!("failed to parse templates file {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !missing_is_error => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(anyhow::anyhow!(
            "templates file not found: {}",
            path.display()
        )),
        Err(e) => Err(anyhow::anyhow!(
            "failed to read templates file {}: {e}",
            path.display()
        )),
    }
}

/// Resolve the database path with `CLI flag > env var > config file > platform default`
/// precedence. The env var is already folded into `cli_db` by clap's `env` attribute on
/// `Cli::db`; the platform-default tier takes its own raw environment values (rather than
/// reading `std::env` internally, like [`default_db_path_from`]) so this stays fully
/// unit-testable without touching real process environment variables.
#[allow(clippy::too_many_arguments)]
pub fn resolve_db_path(
    cli_db: Option<PathBuf>,
    config: &BhtuneConfig,
    xdg_data_home: Option<&str>,
    home: Option<&str>,
    appdata: Option<&str>,
    is_windows: bool,
) -> PathBuf {
    cli_db
        .or_else(|| config.db.clone())
        .unwrap_or_else(|| default_db_path_from(xdg_data_home, home, appdata, is_windows))
}

/// Resolve the opcda-bridge gateway address with `CLI flag > env var > config file >
/// default` precedence. The env var is already folded into `cli_host` by clap's `env`
/// attribute on `TuneArgs::bridge_host`/`OpcCommand`'s per-variant `bridge_host`.
pub fn resolve_bridge_host(cli_host: Option<String>, config: &BhtuneConfig) -> String {
    cli_host
        .or_else(|| config.bridge_host.clone())
        .unwrap_or_else(|| DEFAULT_BRIDGE_HOST.to_string())
}

/// Resolve the history retention policy (`history-retention`) with `CLI flag > env var >
/// config file > default` precedence, matching [`resolve_bridge_host`]'s shape. The env var
/// is already folded into `cli_days` by clap's `env` attribute on `Cli::retention_days`.
/// `None` means retain forever -- there is no built-in default number of days; see
/// [`BhtuneConfig::retention_days`] for why.
pub fn resolve_retention_days(cli_days: Option<u32>, config: &BhtuneConfig) -> Option<u32> {
    cli_days.or(config.retention_days)
}

/// Resolve `bhtune-server`'s bind address with `CLI flag > env var > config file > default`
/// precedence, matching [`resolve_bridge_host`]'s shape exactly. `bhtune-server` has no
/// `clap` dependency (see AGENTS.md's "Deferred setup"), so unlike `resolve_bridge_host` the
/// env var isn't folded in by a derive attribute upstream -- callers pass
/// `std::env::var("BHTUNE_BIND").ok()` (or a real CLI flag, if one is ever added) directly as
/// `cli_bind`.
pub fn resolve_bind_addr(cli_bind: Option<String>, config: &BhtuneConfig) -> String {
    cli_bind
        .or_else(|| config.bind.clone())
        .unwrap_or_else(|| DEFAULT_BIND_ADDR.to_string())
}

pub fn resolve_origin(
    env_origin: Option<String>,
    config: &BhtuneConfig,
    bind_addr: &str,
    mode: ServerMode,
) -> Result<String, String> {
    let origin = env_origin
        .or_else(|| config.origin.clone())
        .unwrap_or_else(|| format!("http://{bind_addr}"));
    if mode == ServerMode::Demo {
        validate_demo_origin(&origin)?;
    }
    Ok(origin)
}

/// Validates the exact browser origin used by a public Demo deployment.
///
/// HTTPS is mandatory except for an explicit loopback HTTP origin used by local tests and
/// development. Origins must contain only a scheme and authority: paths, query strings,
/// fragments, credentials, and trailing slashes are rejected.
pub fn validate_demo_origin(origin: &str) -> Result<(), String> {
    if origin.trim() != origin || origin.chars().any(char::is_whitespace) {
        return Err("demo origin must not contain whitespace".to_owned());
    }
    let (scheme, authority) = origin
        .split_once("://")
        .ok_or_else(|| "demo origin must be an absolute HTTPS origin".to_owned())?;
    if authority.is_empty()
        || authority.contains(['/', '?', '#', '@'])
        || !valid_origin_authority(authority)
    {
        return Err(
            "demo origin must contain only a host and optional port, with no path or credentials"
                .to_owned(),
        );
    }
    match scheme.to_ascii_lowercase().as_str() {
        "https" => Ok(()),
        "http" if authority_host(authority).is_some_and(is_loopback_host) => Ok(()),
        "http" => Err(
            "demo origin must use HTTPS; HTTP is allowed only for an explicit loopback origin"
                .to_owned(),
        ),
        _ => Err("demo origin must use HTTPS".to_owned()),
    }
}

fn valid_origin_authority(authority: &str) -> bool {
    let Some((host, port)) = split_authority(authority) else {
        return false;
    };
    !host.is_empty() && port.is_none_or(|port| !port.is_empty() && port.parse::<u16>().is_ok())
}

fn authority_host(authority: &str) -> Option<&str> {
    split_authority(authority).map(|(host, _)| host)
}

fn split_authority(authority: &str) -> Option<(&str, Option<&str>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let closing = rest.find(']')?;
        let host = &rest[..closing];
        let suffix = &rest[closing + 1..];
        return match suffix {
            "" => Some((host, None)),
            _ => suffix.strip_prefix(':').map(|port| (host, Some(port))),
        };
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Some((host, Some(port))),
        Some(_) => None,
        None => Some((authority, None)),
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// Validates a trusted reverse-proxy address before Demo requests may use forwarded client IPs.
///
/// The request path supports an exact IPv4/IPv6 address or a matching-family CIDR.
pub fn validate_demo_trusted_proxy(trusted_proxy: Option<&str>) -> Result<(), String> {
    let Some(value) = trusted_proxy else {
        return Ok(());
    };
    if value.trim() != value || value.is_empty() {
        return Err("demo trusted_proxy must be a non-empty IP address or CIDR".to_owned());
    }
    if value.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let (network, prefix) = value
        .split_once('/')
        .ok_or_else(|| "demo trusted_proxy must be an IP address or CIDR".to_owned())?;
    let network = network
        .parse::<IpAddr>()
        .map_err(|_| "demo trusted_proxy CIDR must use a valid IP address".to_owned())?;
    let prefix = prefix
        .parse::<u32>()
        .map_err(|_| "demo trusted_proxy CIDR prefix is invalid".to_owned())?;
    let maximum = match network {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    if prefix > maximum {
        return Err(format!(
            "demo trusted_proxy CIDR prefix must be between 0 and {maximum}"
        ));
    }
    Ok(())
}

/// Resolve the OPC DA server ProgID with `CLI flag > config file` precedence, erroring if
/// neither is set -- there's no sensible default for which OPC server to talk to.
pub fn resolve_server(cli_server: Option<String>, config: &BhtuneConfig) -> anyhow::Result<String> {
    cli_server.or_else(|| config.server.clone()).ok_or_else(|| {
        anyhow::anyhow!(
            "no OPC server specified: pass --server or set `server` in the bhtune config file"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::{fs, io::Write};

    proptest::proptest! {
        #[test]
        fn serialized_configs_round_trip(
            db in prop::option::of("[A-Za-z0-9_./:-]{0,32}"),
            bridge_host in prop::option::of("[A-Za-z0-9_.:-]{0,32}"),
            server in prop::option::of("[A-Za-z0-9_.:-]{0,32}"),
            templates in prop::option::of("[A-Za-z0-9_./:-]{0,32}"),
            bind in prop::option::of("[A-Za-z0-9_.:-]{0,32}"),
            retention_days in prop::option::of(1u32..),
            allow_uncertain_quality in any::<bool>(),
            mrft_delay_secs in prop::option::of(0u32..=MAX_TUNING_MRFT_DELAY_SECS),
            poll_interval_ms in prop::option::of(1u64..=1_000_000),
            timeout_secs in prop::option::of(1u64..=1_000_000),
            op_timeout_secs in prop::option::of(1u64..=1_000_000),
            restore_timeout_secs in prop::option::of(1u64..=1_000_000),
            level in prop::option::of("[A-Za-z0-9_.:-]{0,16}"),
            dir in prop::option::of("[A-Za-z0-9_./:-]{0,32}"),
            format in prop::option::of("[A-Za-z0-9_.:-]{0,16}"),
            rotation in prop::option::of("[A-Za-z0-9_.:-]{0,16}"),
        ) {
            let config = BhtuneConfig {
                server_mode: None,
                demo: DemoPolicyConfig::default(),
                db: db.map(PathBuf::from),
                bridge_host,
                server,
                templates: templates.map(PathBuf::from),
                bind,
                origin: None,
                trusted_proxy: None,
                retention_days,
                allow_uncertain_quality,
                tuning: TuningConfig {
                    mrft_delay_secs,
                    poll_interval_ms,
                    timeout_secs,
                    op_timeout_secs,
                    restore_timeout_secs,
                },
                log: LogConfig {
                    level,
                    dir,
                    format,
                    rotation,
                },
            };
            let encoded = toml::to_string(&config).unwrap();
            prop_assert_eq!(parse_config_contents(&encoded).unwrap(), config);
        }

        #[test]
        fn arbitrary_config_text_never_panics(input in any::<String>()) {
            let _ = parse_config_contents(&input);
        }
    }

    #[test]
    fn config_path_from_windows_with_appdata() {
        let path = config_path_from(None, None, Some(r"C:\Users\me\AppData\Roaming"), true);
        assert_eq!(
            path,
            Some(PathBuf::from(
                r"C:\Users\me\AppData\Roaming/bhtune/bhtune.toml"
            ))
        );
    }

    #[test]
    fn parse_config_rejects_zero_retention_days() {
        let error = parse_config_contents("retention_days = 0").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("retention_days must be at least 1")
        );
    }

    #[test]
    fn missing_tuning_table_preserves_none_and_resolves_built_in_defaults() {
        let config = parse_config_contents("bridge_host = \"gateway:7600\"\n").unwrap();

        assert_eq!(config.tuning, TuningConfig::default());
        assert_eq!(
            resolve_and_validate_tuning_config(&config.tuning, true).unwrap(),
            EffectiveTuningConfig::default()
        );
    }

    #[test]
    fn tuning_table_round_trips_all_optional_values() {
        let raw = r#"
[tuning]
mrft_delay_secs = 12
poll_interval_ms = 900
timeout_secs = 4000
op_timeout_secs = 31
restore_timeout_secs = 32
"#;
        let config = parse_config_contents(raw).unwrap();

        assert_eq!(
            config.tuning,
            TuningConfig {
                mrft_delay_secs: Some(12),
                poll_interval_ms: Some(900),
                timeout_secs: Some(4_000),
                op_timeout_secs: Some(31),
                restore_timeout_secs: Some(32),
            }
        );
        let encoded = toml::to_string(&config).unwrap();
        assert_eq!(parse_config_contents(&encoded).unwrap(), config);
    }

    #[test]
    fn tuning_resolution_preserves_partial_raw_values_and_tracks_sources() {
        let raw = TuningConfig {
            poll_interval_ms: Some(250),
            restore_timeout_secs: Some(8),
            ..Default::default()
        };

        assert_eq!(
            resolve_tuning_config(&raw),
            EffectiveTuningConfig {
                poll_interval_ms: 250,
                restore_timeout_secs: 8,
                ..Default::default()
            }
        );
        assert_eq!(
            tuning_config_sources(&raw),
            TuningConfigSources {
                mrft_delay_secs: TuningConfigSource::BuiltInDefault,
                poll_interval_ms: TuningConfigSource::Toml,
                timeout_secs: TuningConfigSource::BuiltInDefault,
                op_timeout_secs: TuningConfigSource::BuiltInDefault,
                restore_timeout_secs: TuningConfigSource::Toml,
            }
        );
    }

    #[test]
    fn tuning_validation_rejects_each_invalid_general_value() {
        let cases = [
            (
                EffectiveTuningConfig {
                    mrft_delay_secs: MAX_TUNING_MRFT_DELAY_SECS + 1,
                    ..Default::default()
                },
                TuningConfigError::MrftDelayOutOfRange {
                    value: MAX_TUNING_MRFT_DELAY_SECS + 1,
                },
            ),
            (
                EffectiveTuningConfig {
                    poll_interval_ms: 0,
                    ..Default::default()
                },
                TuningConfigError::PollIntervalTooSmall { value: 0 },
            ),
            (
                EffectiveTuningConfig {
                    timeout_secs: 0,
                    ..Default::default()
                },
                TuningConfigError::TimeoutTooSmall { value: 0 },
            ),
            (
                EffectiveTuningConfig {
                    op_timeout_secs: 0,
                    ..Default::default()
                },
                TuningConfigError::OpTimeoutTooSmall { value: 0 },
            ),
            (
                EffectiveTuningConfig {
                    restore_timeout_secs: 0,
                    ..Default::default()
                },
                TuningConfigError::RestoreTimeoutTooSmall {
                    value: 0,
                    minimum: 1,
                },
            ),
        ];

        for (config, expected) in cases {
            assert_eq!(validate_tuning_config(&config, false), Err(expected));
            assert!(!expected.to_string().is_empty());
        }
    }

    #[test]
    fn opc_restore_timeout_requires_four_seconds_but_general_validation_allows_one() {
        let config = EffectiveTuningConfig {
            restore_timeout_secs: MIN_OPC_RESTORE_TIMEOUT_SECS - 1,
            ..Default::default()
        };

        assert_eq!(validate_tuning_config(&config, false), Ok(()));
        assert_eq!(
            validate_tuning_config(&config, true),
            Err(TuningConfigError::RestoreTimeoutTooSmall {
                value: MIN_OPC_RESTORE_TIMEOUT_SECS - 1,
                minimum: MIN_OPC_RESTORE_TIMEOUT_SECS,
            })
        );
        assert!(
            validate_tuning_config(
                &EffectiveTuningConfig {
                    restore_timeout_secs: MIN_OPC_RESTORE_TIMEOUT_SECS,
                    ..Default::default()
                },
                true,
            )
            .is_ok()
        );
    }

    #[test]
    fn config_path_from_windows_no_appdata() {
        assert_eq!(
            config_path_from(Some("/xdg"), Some("/home"), None, true),
            None
        );
    }

    #[test]
    fn config_path_from_unix_xdg_config_home() {
        let path = config_path_from(Some("/xdg"), Some("/home/me"), None, false);
        assert_eq!(path, Some(PathBuf::from("/xdg/bhtune/bhtune.toml")));
    }

    #[test]
    fn config_path_from_unix_falls_back_to_home() {
        let path = config_path_from(None, Some("/home/me"), None, false);
        assert_eq!(
            path,
            Some(PathBuf::from("/home/me/.config/bhtune/bhtune.toml"))
        );
    }

    #[test]
    fn config_path_from_unix_no_env_vars() {
        assert_eq!(config_path_from(None, None, None, false), None);
    }

    #[test]
    fn config_path_from_unix_xdg_takes_precedence_over_home() {
        let path = config_path_from(Some("/xdg"), Some("/home/me"), None, false);
        assert_eq!(path, Some(PathBuf::from("/xdg/bhtune/bhtune.toml")));
    }

    #[test]
    fn templates_path_from_windows_with_appdata() {
        let path = templates_path_from(None, None, Some(r"C:\Users\me\AppData\Roaming"), true);
        assert_eq!(
            path,
            Some(PathBuf::from(
                r"C:\Users\me\AppData\Roaming/bhtune/templates.toml"
            ))
        );
    }

    #[test]
    fn templates_path_from_windows_no_appdata() {
        assert_eq!(
            templates_path_from(Some("/xdg"), Some("/home"), None, true),
            None
        );
    }

    #[test]
    fn templates_path_from_unix_xdg_config_home() {
        let path = templates_path_from(Some("/xdg"), Some("/home/me"), None, false);
        assert_eq!(path, Some(PathBuf::from("/xdg/bhtune/templates.toml")));
    }

    #[test]
    fn templates_path_from_unix_falls_back_to_home() {
        let path = templates_path_from(None, Some("/home/me"), None, false);
        assert_eq!(
            path,
            Some(PathBuf::from("/home/me/.config/bhtune/templates.toml"))
        );
    }

    #[test]
    fn templates_path_from_unix_no_env_vars() {
        assert_eq!(templates_path_from(None, None, None, false), None);
    }

    #[test]
    fn default_db_path_from_windows_with_appdata() {
        let path = default_db_path_from(None, None, Some(r"C:\Users\me\AppData\Roaming"), true);
        assert_eq!(
            path,
            PathBuf::from(r"C:\Users\me\AppData\Roaming/bhtune/bhtune.db")
        );
    }

    #[test]
    fn default_db_path_from_windows_no_appdata_falls_back_to_cwd() {
        assert_eq!(
            default_db_path_from(None, None, None, true),
            PathBuf::from("bhtune.db")
        );
    }

    #[test]
    fn default_db_path_from_unix_xdg_data_home() {
        let path = default_db_path_from(Some("/xdg-data"), Some("/home/me"), None, false);
        assert_eq!(path, PathBuf::from("/xdg-data/bhtune/bhtune.db"));
    }

    #[test]
    fn default_db_path_from_unix_falls_back_to_home() {
        let path = default_db_path_from(None, Some("/home/me"), None, false);
        assert_eq!(
            path,
            PathBuf::from("/home/me/.local/share/bhtune/bhtune.db")
        );
    }

    #[test]
    fn default_db_path_from_unix_no_env_vars_falls_back_to_cwd() {
        assert_eq!(
            default_db_path_from(None, None, None, false),
            PathBuf::from("bhtune.db")
        );
    }

    #[test]
    fn default_log_dir_from_windows_with_appdata() {
        let path = default_log_dir_from(None, None, Some(r"C:\Users\me\AppData\Roaming"), true);
        assert_eq!(
            path,
            PathBuf::from(r"C:\Users\me\AppData\Roaming/bhtune/logs")
        );
    }

    #[test]
    fn default_log_dir_from_windows_no_appdata_falls_back_to_cwd() {
        assert_eq!(
            default_log_dir_from(None, None, None, true),
            PathBuf::from("logs")
        );
    }

    #[test]
    fn default_log_dir_from_unix_xdg_data_home() {
        let path = default_log_dir_from(Some("/xdg-data"), Some("/home/me"), None, false);
        assert_eq!(path, PathBuf::from("/xdg-data/bhtune/logs"));
    }

    #[test]
    fn default_log_dir_from_unix_falls_back_to_home() {
        let path = default_log_dir_from(None, Some("/home/me"), None, false);
        assert_eq!(path, PathBuf::from("/home/me/.local/share/bhtune/logs"));
    }

    #[test]
    fn default_log_dir_from_unix_no_env_vars_falls_back_to_cwd() {
        assert_eq!(
            default_log_dir_from(None, None, None, false),
            PathBuf::from("logs")
        );
    }

    #[test]
    fn default_allow_uncertain_quality_is_true() {
        assert!(BhtuneConfig::default().allow_uncertain_quality);
    }

    #[test]
    fn load_config_file_missing_allow_uncertain_quality_key_defaults_to_true() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "bridge_host = \"gateway:7600\"").unwrap();
        let config = load_config_file(file.path(), true).unwrap();
        assert!(config.allow_uncertain_quality);
        assert_eq!(config.bridge_host, Some("gateway:7600".to_string()));
    }

    #[test]
    fn resolve_config_store_path_prefers_an_explicit_path() {
        let resolution = resolve_config_store_path(
            Some(Path::new("/explicit/bhtune.toml")),
            Some("/xdg"),
            Some("/home/me"),
            None,
            false,
        );
        assert_eq!(
            resolution,
            ConfigPathResolution {
                path: Some(PathBuf::from("/explicit/bhtune.toml")),
                missing_is_allowed: false,
            }
        );
    }

    #[test]
    fn resolve_config_store_path_falls_back_to_auto_discovery() {
        let resolution =
            resolve_config_store_path(None, Some("/xdg"), Some("/home/me"), None, false);
        assert_eq!(
            resolution,
            ConfigPathResolution {
                path: Some(PathBuf::from("/xdg/bhtune/bhtune.toml")),
                missing_is_allowed: true,
            }
        );
    }

    #[test]
    fn load_config_file_valid() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "db = \"/data/bhtune.db\"\nbridge_host = \"gateway:7600\"\nserver = \"Kepware.KEPServerEX.V6\""
        )
        .unwrap();
        let config = load_config_file(file.path(), true).unwrap();
        assert_eq!(config.db, Some(PathBuf::from("/data/bhtune.db")));
        assert_eq!(config.bridge_host, Some("gateway:7600".to_string()));
        assert_eq!(config.server, Some("Kepware.KEPServerEX.V6".to_string()));
        assert_eq!(config.log, LogConfig::default());
    }

    #[test]
    fn load_config_file_parses_the_log_table() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "[log]\nlevel = \"debug\"\ndir = \"/var/log/bhtune\"\nformat = \"json\"\nrotation = \"hourly\""
        )
        .unwrap();
        let config = load_config_file(file.path(), true).unwrap();
        assert_eq!(
            config.log,
            LogConfig {
                level: Some("debug".to_string()),
                dir: Some("/var/log/bhtune".to_string()),
                format: Some("json".to_string()),
                rotation: Some("hourly".to_string()),
            }
        );
    }

    #[test]
    fn load_config_file_empty_is_all_defaults() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let config = load_config_file(file.path(), true).unwrap();
        assert_eq!(config, BhtuneConfig::default());
    }

    #[test]
    fn load_config_file_malformed() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "db = 12345").unwrap();
        let err = load_config_file(file.path(), true).unwrap_err();
        assert!(err.to_string().contains("failed to parse config file"));
    }

    #[test]
    fn load_config_file_missing_not_error() {
        let config = load_config_file(Path::new("/nonexistent/bhtune.toml"), false).unwrap();
        assert_eq!(config, BhtuneConfig::default());
    }

    #[test]
    fn load_config_file_missing_is_error() {
        let err = load_config_file(Path::new("/nonexistent/bhtune.toml"), true).unwrap_err();
        assert!(err.to_string().contains("config file not found"));
    }

    #[test]
    fn load_config_file_generic_io_error() {
        // Reading a directory as a file fails with an `IsADirectory`-style error, distinct
        // from `NotFound` -- exercises the catch-all I/O error branch (e.g. permission
        // denied in real usage).
        let dir = tempfile::tempdir().unwrap();
        let err = load_config_file(dir.path(), true).unwrap_err();
        assert!(err.to_string().contains("failed to read config file"));
    }

    #[test]
    fn load_config_explicit_path() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "bridge_host = \"custom:9999\"").unwrap();
        let config = load_config(Some(file.path())).unwrap();
        assert_eq!(config.bridge_host, Some("custom:9999".to_string()));
    }

    #[test]
    fn load_config_explicit_path_missing_errors() {
        let err = load_config(Some(Path::new("/nonexistent/bhtune.toml"))).unwrap_err();
        assert!(err.to_string().contains("config file not found"));
    }

    #[test]
    fn load_config_default_discovery() {
        // No file will exist next to the test binary's own executable path, so this
        // exercises the "missing, not an error" auto-discovery path against whatever the
        // real test machine's environment happens to be.
        let config = load_config(None).unwrap();
        assert_eq!(config, BhtuneConfig::default());
    }

    #[test]
    fn load_discovered_config_with_no_path_found_is_all_defaults() {
        // Exercises the "neither XDG_CONFIG_HOME nor HOME is set" case directly, without
        // mutating real process environment variables (unsafe/flaky in a parallel test
        // binary) to force `config_path_from` itself to return `None`.
        let config = load_discovered_config(None).unwrap();
        assert_eq!(config, BhtuneConfig::default());
    }

    #[test]
    fn load_discovered_config_reads_a_valid_discovered_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        fs::write(file.path(), "bridge_host = \"discovered:7600\"\n").unwrap();

        let config = load_discovered_config(Some(file.path().to_path_buf())).unwrap();

        assert_eq!(config.bridge_host, Some("discovered:7600".to_string()));
    }

    #[test]
    fn revision_hash_uses_the_stable_fnv1a_algorithm() {
        assert_eq!(stable_revision_hash(b"bhtune"), 0xeeeb3aadbd6c2361);
    }

    #[test]
    fn unique_suffix_has_process_and_timestamp_components() {
        let suffix = unique_suffix();
        let components: Vec<_> = suffix.split('-').collect();

        assert_eq!(components.len(), 3);
        assert_eq!(components[0], std::process::id().to_string());
        assert!(components[1].parse::<u64>().is_ok());
        assert!(
            components[2]
                .parse::<u32>()
                .is_ok_and(|n| n < 1_000_000_000)
        );
    }

    fn backup_and_temp_siblings(path: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let parent = path.parent().unwrap();
        let file_name = path.file_name().unwrap().to_string_lossy().to_string();
        let mut backups = Vec::new();
        let mut temps = Vec::new();

        for entry in fs::read_dir(parent).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("{file_name}.backup-")) {
                backups.push(entry.path());
            }
            if name.starts_with(&format!("{file_name}.tmp-")) {
                temps.push(entry.path());
            }
        }

        backups.sort();
        temps.sort();
        (backups, temps)
    }

    #[test]
    fn backup_and_temp_siblings_finds_temporary_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        let temp_path = dir.path().join("bhtune.toml.tmp-leftover");
        fs::write(&temp_path, b"leftover").unwrap();

        let (_backups, temps) = backup_and_temp_siblings(&path);

        assert_eq!(temps, vec![temp_path]);
    }

    #[test]
    fn load_config_store_from_missing_auto_path_returns_a_path_aware_default_store() {
        let dir = tempfile::tempdir().unwrap();
        let store =
            load_config_store_from(None, Some(dir.path().to_str().unwrap()), None, None, false)
                .unwrap();

        assert_eq!(
            store.path,
            Some(dir.path().join("bhtune").join("bhtune.toml"))
        );
        assert!(store.missing_is_allowed);
        assert_eq!(store.original_raw, None);
        assert_eq!(store.config, BhtuneConfig::default());
        assert_eq!(store.revision, "absent:v1");
        assert_eq!(store.toml_tuning, TuningConfig::default());
        assert_eq!(
            store.tuning_sources,
            tuning_config_sources(&TuningConfig::default())
        );
    }

    #[test]
    fn load_config_store_tracks_raw_tuning_values_and_sources() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "[tuning]\npoll_interval_ms = 250\nrestore_timeout_secs = 8"
        )
        .unwrap();

        let store = load_config_store(Some(file.path())).unwrap();

        assert_eq!(
            store.toml_tuning,
            TuningConfig {
                poll_interval_ms: Some(250),
                restore_timeout_secs: Some(8),
                ..Default::default()
            }
        );
        assert_eq!(
            store.tuning_sources,
            TuningConfigSources {
                mrft_delay_secs: TuningConfigSource::BuiltInDefault,
                poll_interval_ms: TuningConfigSource::Toml,
                timeout_secs: TuningConfigSource::BuiltInDefault,
                op_timeout_secs: TuningConfigSource::BuiltInDefault,
                restore_timeout_secs: TuningConfigSource::Toml,
            }
        );
    }

    #[test]
    fn load_config_store_wrapper_uses_the_explicit_path() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let store = load_config_store(Some(file.path())).unwrap();
        assert_eq!(store.path, Some(file.path().to_path_buf()));
        assert_eq!(store.config, BhtuneConfig::default());
    }

    #[test]
    fn load_config_store_from_explicit_missing_path_is_a_typed_error() {
        let path = PathBuf::from("/nonexistent/path-aware-bhtune.toml");
        let err = load_config_store_from(Some(&path), None, None, None, false).unwrap_err();
        assert!(matches!(err, ConfigStoreError::Missing { path: actual } if actual == path));
    }

    #[test]
    fn load_config_store_from_malformed_input_is_a_typed_error() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "db = 12345").unwrap();

        let err = load_config_store_from(Some(file.path()), None, None, None, false).unwrap_err();
        assert!(matches!(
            err,
            ConfigStoreError::Malformed {
                path: Some(path), ..
            } if path == file.path()
        ));
    }

    #[test]
    fn load_config_store_from_unreadable_path_is_a_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_config_store_from(Some(dir.path()), None, None, None, false).unwrap_err();
        assert!(matches!(
            err,
            ConfigStoreError::Unreadable { path, .. } if path == dir.path()
        ));
    }

    #[test]
    fn load_config_store_from_rejects_non_utf8_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        fs::write(&path, [0xff, 0xfe]).unwrap();

        let err = load_config_store_from(Some(&path), None, None, None, false).unwrap_err();
        let message = err.to_string();
        assert!(matches!(
            err,
            ConfigStoreError::Malformed {
                path: Some(actual), ..
            } if actual == path
        ));
        assert!(message.contains("not valid UTF-8"));
    }

    #[test]
    fn patch_helpers_preserve_comments_unknown_keys_and_unrelated_values() {
        let raw = r#"# keep this comment
bridge_host = "gateway:7600"
unknown_key = "keep me"

[log]
level = "info"
"#;

        let patched = patch_allow_uncertain_quality(Some(raw), false).unwrap();
        let patched = patch_retention_days(Some(&patched), Some(30)).unwrap();

        assert!(patched.contains("# keep this comment"));
        assert!(patched.contains("bridge_host = \"gateway:7600\""));
        assert!(patched.contains("unknown_key = \"keep me\""));
        assert!(patched.contains("[log]"));
        assert!(patched.contains("level = \"info\""));
        assert!(patched.contains("allow_uncertain_quality = false"));
        assert!(patched.contains("retention_days = 30"));

        let parsed = parse_config_contents(&patched).unwrap();
        assert_eq!(parsed.bridge_host, Some("gateway:7600".to_string()));
        assert_eq!(parsed.log.level, Some("info".to_string()));
        assert!(!parsed.allow_uncertain_quality);
        assert_eq!(parsed.retention_days, Some(30));
    }

    #[test]
    fn patch_retention_days_removes_an_existing_key() {
        let patched = patch_retention_days(Some("retention_days = 30\n"), None).unwrap();
        assert!(!patched.contains("retention_days"));
        assert_eq!(
            parse_config_contents(&patched).unwrap().retention_days,
            None
        );
    }

    #[test]
    fn patch_tuning_config_updates_values_and_preserves_comments_and_unknown_keys() {
        let raw = r#"# keep root comment
unknown_key = "keep me"

[tuning]
# keep tuning comment
mrft_delay_secs = 1
unknown_tuning_key = "keep this too"
"#;
        let patched = patch_tuning_config(
            Some(raw),
            &TuningConfig {
                mrft_delay_secs: Some(10),
                poll_interval_ms: Some(250),
                timeout_secs: Some(900),
                op_timeout_secs: Some(5),
                restore_timeout_secs: Some(6),
            },
        )
        .unwrap();

        assert!(patched.contains("# keep root comment"));
        assert!(patched.contains("# keep tuning comment"));
        assert!(patched.contains("unknown_key = \"keep me\""));
        assert!(patched.contains("unknown_tuning_key = \"keep this too\""));
        assert_eq!(
            parse_config_contents(&patched).unwrap().tuning,
            TuningConfig {
                mrft_delay_secs: Some(10),
                poll_interval_ms: Some(250),
                timeout_secs: Some(900),
                op_timeout_secs: Some(5),
                restore_timeout_secs: Some(6),
            }
        );
    }

    #[test]
    fn patch_tuning_config_none_removes_all_managed_keys_but_keeps_unknown_content() {
        let raw = r#"
[tuning]
mrft_delay_secs = 10
poll_interval_ms = 250
timeout_secs = 900
op_timeout_secs = 5
restore_timeout_secs = 6
unknown_tuning_key = "keep"
"#;

        let patched = patch_tuning_config(Some(raw), &TuningConfig::default()).unwrap();

        for key in [
            "mrft_delay_secs",
            "poll_interval_ms",
            "timeout_secs",
            "op_timeout_secs",
            "restore_timeout_secs",
        ] {
            assert!(!patched.contains(key));
        }
        assert!(patched.contains("unknown_tuning_key = \"keep\""));
        assert_eq!(
            parse_config_contents(&patched).unwrap().tuning,
            TuningConfig::default()
        );
    }

    #[test]
    fn patch_tuning_config_rejects_invalid_and_unrepresentable_values() {
        let invalid = patch_tuning_config(
            None,
            &TuningConfig {
                poll_interval_ms: Some(0),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(invalid.to_string().contains("poll_interval_ms"));

        let too_large = patch_tuning_config(
            None,
            &TuningConfig {
                timeout_secs: Some(u64::MAX),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(too_large.to_string().contains("too large"));
    }

    #[test]
    fn patch_helpers_report_malformed_toml_without_modifying_it() {
        let malformed = "not = [valid";

        let quality_error = patch_allow_uncertain_quality(Some(malformed), false).unwrap_err();
        assert!(matches!(
            quality_error,
            ConfigStoreError::Malformed { path: None, .. }
        ));

        let retention_error = patch_retention_days(Some(malformed), Some(7)).unwrap_err();
        assert!(matches!(
            retention_error,
            ConfigStoreError::Malformed { path: None, .. }
        ));
    }

    #[test]
    fn patch_policy_reports_malformed_toml_without_a_path() {
        let error = patch_config_policy(
            Some("not = [valid"),
            &ConfigPolicyUpdate {
                allow_uncertain_quality: false,
                retention_days: Some(7),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(!error.is_empty());
    }

    #[test]
    fn generated_sibling_names_fall_back_when_a_path_has_no_file_name() {
        let path = Path::new("");
        assert!(
            sibling_with_suffix(path, "tmp")
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("bhtune.toml.tmp-")
        );
        assert!(
            backup_path_for(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("bhtune.toml.backup-")
        );
    }

    #[test]
    fn discovered_config_propagates_an_unreadable_path() {
        let error = load_discovered_config(Some(PathBuf::from("."))).unwrap_err();
        assert!(error.to_string().contains("failed to read config file"));
    }

    #[test]
    fn atomic_writer_reports_a_backup_copy_failure_from_the_real_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let error = write_config_file_atomically(dir.path(), b"replacement", false).unwrap_err();
        assert!(matches!(
            error,
            ConfigStoreError::Write {
                action: "create config backup",
                ..
            }
        ));
    }

    #[test]
    fn patch_config_policy_removes_an_existing_retention_key() {
        let (patched, config) = patch_config_policy(
            Some("allow_uncertain_quality = true\nretention_days = 30\n"),
            &ConfigPolicyUpdate {
                allow_uncertain_quality: false,
                retention_days: None,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!patched.contains("retention_days"));
        assert!(!config.allow_uncertain_quality);
        assert_eq!(config.retention_days, None);
    }

    #[test]
    fn config_store_error_display_and_sources_cover_all_variants() {
        let path = PathBuf::from("/tmp/bhtune.toml");
        let errors = [
            ConfigStoreError::PathNotResolved,
            ConfigStoreError::Missing { path: path.clone() },
            ConfigStoreError::Unreadable {
                path: path.clone(),
                source: io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
            },
            ConfigStoreError::Malformed {
                path: Some(path.clone()),
                source: "bad".to_string(),
            },
            ConfigStoreError::Malformed {
                path: None,
                source: "bad".to_string(),
            },
            ConfigStoreError::Conflict {
                path: Some(path.clone()),
                message: "stale".to_string(),
            },
            ConfigStoreError::Conflict {
                path: None,
                message: "stale".to_string(),
            },
            ConfigStoreError::Write {
                path,
                action: "write config",
                source: io::Error::other("failed"),
            },
        ];

        for error in errors {
            assert!(!error.to_string().is_empty());
            let has_source = std::error::Error::source(&error).is_some();
            assert_eq!(
                has_source,
                matches!(
                    error,
                    ConfigStoreError::Unreadable { .. } | ConfigStoreError::Write { .. }
                )
            );
        }
    }

    #[test]
    fn create_temp_file_retries_after_a_name_collision() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        let collision = dir.path().join("bhtune.toml.tmp-collision");
        let available = dir.path().join("bhtune.toml.tmp-available");
        fs::write(&collision, b"already here").unwrap();

        let mut candidates = vec![collision.clone(), available.clone()];
        let (created, file) = create_temp_file_with(&path, || candidates.remove(0)).unwrap();
        assert_eq!(created, available);
        drop(file);
        assert!(created.exists());
        fs::remove_file(created).unwrap();
    }

    #[test]
    fn create_temp_file_reports_exhausted_name_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        let collision = dir.path().join("bhtune.toml.tmp-collision");
        fs::write(&collision, b"already here").unwrap();

        let err = create_temp_file_with(&path, || collision.clone()).unwrap_err();
        assert!(matches!(
            err,
            ConfigStoreError::Write { source, .. }
                if source.kind() == io::ErrorKind::AlreadyExists
        ));
    }

    #[test]
    fn create_temp_file_reports_non_collision_errors_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-directory");
        fs::write(&blocker, b"file").unwrap();
        let path = dir.path().join("bhtune.toml");
        let candidate = blocker.join("bhtune.toml.tmp");

        let err = create_temp_file_with(&path, || candidate.clone()).unwrap_err();

        assert!(matches!(
            err,
            ConfigStoreError::Write { source, .. }
                if source.kind() != io::ErrorKind::AlreadyExists
        ));
    }

    struct TestTempFile {
        bytes: Vec<u8>,
        fail_write: bool,
        fail_sync: bool,
    }

    impl Write for TestTempFile {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail_write {
                Err(io::Error::other("write failed"))
            } else {
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl SyncConfigFile for TestTempFile {
        fn sync_config(&self) -> io::Result<()> {
            if self.fail_sync {
                Err(io::Error::other("sync failed"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn write_and_flush_temp_file_reports_write_and_sync_failures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        let write_temp = dir.path().join("write.tmp");
        fs::write(&write_temp, b"placeholder").unwrap();
        let mut writer = TestTempFile {
            bytes: Vec::new(),
            fail_write: true,
            fail_sync: false,
        };
        writer.flush().unwrap();
        let err =
            write_and_flush_temp_file(&path, &write_temp, b"config", &mut writer).unwrap_err();
        assert!(matches!(
            err,
            ConfigStoreError::Write { action, .. } if action == "write temporary config file"
        ));
        assert!(!write_temp.exists());

        let sync_temp = dir.path().join("sync.tmp");
        fs::write(&sync_temp, b"placeholder").unwrap();
        let mut writer = TestTempFile {
            bytes: Vec::new(),
            fail_write: false,
            fail_sync: true,
        };
        let err = write_and_flush_temp_file(&path, &sync_temp, b"config", &mut writer).unwrap_err();
        assert!(matches!(
            err,
            ConfigStoreError::Write { action, .. } if action == "flush temporary config file"
        ));
        assert!(!sync_temp.exists());
    }

    #[test]
    fn atomic_writer_reports_parent_and_temp_creation_failures() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        fs::write(&blocker, b"not a directory").unwrap();
        let target = blocker.join("bhtune.toml");

        let err = write_config_file_atomically(&target, b"config", true).unwrap_err();
        assert!(matches!(
            err,
            ConfigStoreError::Write { action, .. } if action == "create config directory"
        ));

        let err = write_config_file_atomically(&target, b"config", false).unwrap_err();
        assert!(matches!(
            err,
            ConfigStoreError::Write { action, .. } if action == "create temporary config file"
        ));
    }

    #[test]
    fn atomic_writer_reports_backup_and_replace_failures() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("existing.toml");
        fs::write(&existing, b"old").unwrap();
        let noop_replace = |_source: &Path, _destination: &Path| Ok(());
        let err = write_config_file_atomically_with(
            &existing,
            b"new",
            false,
            |_source, _destination| Err(io::Error::other("backup failed")),
            noop_replace,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigStoreError::Write { action, .. } if action == "create config backup"
        ));
        assert_eq!(fs::read(&existing).unwrap(), b"old");

        let successful_target = dir.path().join("successful.toml");
        fs::write(&successful_target, b"old").unwrap();
        let successful_backup = write_config_file_atomically_with(
            &successful_target,
            b"new",
            false,
            |_source, _destination| Ok(0),
            noop_replace,
        )
        .unwrap();
        assert!(successful_backup.is_some());
        assert_eq!(fs::read(&successful_target).unwrap(), b"old");

        let replace_target = dir.path().join("replace.toml");
        fs::write(&replace_target, b"old").unwrap();
        let err = write_config_file_atomically_with(
            &replace_target,
            b"new",
            false,
            |_source, _destination| Ok(0),
            |_source, _destination| Err(io::Error::other("replace failed")),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigStoreError::Write { action, .. } if action == "replace config file"
        ));
        assert_eq!(fs::read(&replace_target).unwrap(), b"old");

        let mut successful_sync = TestTempFile {
            bytes: Vec::new(),
            fail_write: false,
            fail_sync: false,
        };
        write_and_flush_temp_file(
            &replace_target,
            &dir.path().join("successful.tmp"),
            b"config",
            &mut successful_sync,
        )
        .unwrap();
        assert_eq!(successful_sync.bytes, b"config");
        successful_sync.sync_config().unwrap();
    }

    #[test]
    fn save_config_store_creates_a_missing_auto_discovered_file_and_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let store =
            load_config_store_from(None, Some(dir.path().to_str().unwrap()), None, None, false)
                .unwrap();
        let expected_path = dir.path().join("bhtune").join("bhtune.toml");

        let result = save_config_store(
            &store,
            &store.revision,
            &ConfigPolicyUpdate {
                allow_uncertain_quality: false,
                retention_days: Some(14),
                mrft_delay_secs: Some(12),
                poll_interval_ms: Some(250),
                timeout_secs: Some(900),
                op_timeout_secs: Some(5),
                restore_timeout_secs: Some(6),
            },
        )
        .unwrap();

        assert_eq!(result.backup_path, None);
        assert!(expected_path.exists());
        assert_eq!(result.state.path, Some(expected_path.clone()));
        assert_eq!(result.state.config.retention_days, Some(14));
        assert!(!result.state.config.allow_uncertain_quality);
        assert_eq!(
            result.state.config.tuning,
            TuningConfig {
                mrft_delay_secs: Some(12),
                poll_interval_ms: Some(250),
                timeout_secs: Some(900),
                op_timeout_secs: Some(5),
                restore_timeout_secs: Some(6),
            }
        );
        assert_eq!(result.state.toml_tuning, result.state.config.tuning);
        assert_eq!(
            result.state.tuning_sources,
            tuning_config_sources(&result.state.toml_tuning)
        );
        let saved = fs::read_to_string(expected_path).unwrap();
        assert_eq!(result.state.original_raw.as_deref(), Some(saved.as_str()));
    }

    #[test]
    fn save_config_store_creates_a_timestamped_backup_for_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        let original = "bridge_host = \"before:7600\"\nretention_days = 7\n";
        fs::write(&path, original).unwrap();

        let store = load_config_store_from(Some(&path), None, None, None, false).unwrap();
        let result = save_config_store(
            &store,
            &store.revision,
            &ConfigPolicyUpdate {
                allow_uncertain_quality: false,
                retention_days: Some(21),
                ..Default::default()
            },
        )
        .unwrap();

        let backup_path = result.backup_path.clone().unwrap();
        assert!(backup_path.exists());
        assert_eq!(fs::read_to_string(backup_path).unwrap(), original);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            result.state.original_raw.unwrap()
        );
    }

    #[test]
    fn save_config_store_replaces_the_target_file_and_cleans_up_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        fs::write(&path, "bridge_host = \"before:7600\"\n").unwrap();

        let store = load_config_store_from(Some(&path), None, None, None, false).unwrap();
        let result = save_config_store(
            &store,
            &store.revision,
            &ConfigPolicyUpdate {
                allow_uncertain_quality: true,
                retention_days: Some(9),
                ..Default::default()
            },
        )
        .unwrap();

        let final_raw = fs::read_to_string(&path).unwrap();
        assert_eq!(final_raw, result.state.original_raw.unwrap());
        assert!(final_raw.contains("allow_uncertain_quality = true"));
        assert!(final_raw.contains("retention_days = 9"));
        let (_backups, temps) = backup_and_temp_siblings(&path);
        assert!(temps.is_empty(), "temporary config files were left behind");
    }

    #[test]
    fn save_config_store_rejects_a_stale_revision_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        fs::write(&path, "bridge_host = \"before:7600\"\n").unwrap();

        let store = load_config_store_from(Some(&path), None, None, None, false).unwrap();
        let err = save_config_store(
            &store,
            "present:v1:stale",
            &ConfigPolicyUpdate {
                allow_uncertain_quality: false,
                retention_days: Some(5),
                poll_interval_ms: Some(250),
                ..Default::default()
            },
        )
        .unwrap_err();

        assert!(matches!(
            err,
            ConfigStoreError::Conflict { message, .. }
                if message.contains("stale config revision token")
        ));
    }

    #[test]
    fn save_config_store_resets_tuning_keys_without_removing_unknown_tuning_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        fs::write(
            &path,
            "[tuning]\npoll_interval_ms = 250\nrestore_timeout_secs = 8\nunknown = \"keep\"\n",
        )
        .unwrap();
        let store = load_config_store(Some(&path)).unwrap();

        let result =
            save_config_store(&store, &store.revision, &ConfigPolicyUpdate::default()).unwrap();

        let saved = fs::read_to_string(path).unwrap();
        assert!(!saved.contains("poll_interval_ms"));
        assert!(!saved.contains("restore_timeout_secs"));
        assert!(saved.contains("unknown = \"keep\""));
        assert_eq!(result.state.toml_tuning, TuningConfig::default());
        assert_eq!(result.state.config.tuning, TuningConfig::default());
    }

    #[test]
    fn save_config_store_rejects_invalid_tuning_updates_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        let original = "bridge_host = \"before:7600\"\n";
        fs::write(&path, original).unwrap();
        let store = load_config_store(Some(&path)).unwrap();

        let error = save_config_store(
            &store,
            &store.revision,
            &ConfigPolicyUpdate {
                poll_interval_ms: Some(0),
                ..Default::default()
            },
        )
        .unwrap_err();

        assert!(matches!(error, ConfigStoreError::Malformed { .. }));
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn save_config_store_rejects_external_disk_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.toml");
        fs::write(&path, "bridge_host = \"before:7600\"\n").unwrap();

        let store = load_config_store_from(Some(&path), None, None, None, false).unwrap();
        fs::write(&path, "bridge_host = \"outside:7600\"\n").unwrap();

        let err = save_config_store(
            &store,
            &store.revision,
            &ConfigPolicyUpdate {
                allow_uncertain_quality: false,
                retention_days: Some(5),
                ..Default::default()
            },
        )
        .unwrap_err();

        assert!(matches!(
            err,
            ConfigStoreError::Conflict { message, .. }
                if message.contains("changed on disk since it was loaded")
        ));
    }

    #[test]
    fn save_config_store_rejects_unresolved_and_explicit_missing_paths() {
        let unresolved = load_config_store_from(None, None, None, None, false).unwrap();
        let err = save_config_store(
            &unresolved,
            &unresolved.revision,
            &ConfigPolicyUpdate {
                allow_uncertain_quality: true,
                retention_days: None,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, ConfigStoreError::PathNotResolved));

        let path = PathBuf::from("/nonexistent/explicit-save-config.toml");
        let state = LoadedConfigStore {
            path: Some(path.clone()),
            missing_is_allowed: false,
            original_raw: None,
            config: BhtuneConfig::default(),
            revision: revision_token_for_raw(None),
            toml_allow_uncertain_quality: None,
            toml_tuning: TuningConfig::default(),
            tuning_sources: tuning_config_sources(&TuningConfig::default()),
        };
        let err = save_config_store(
            &state,
            &state.revision,
            &ConfigPolicyUpdate {
                allow_uncertain_quality: true,
                retention_days: None,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, ConfigStoreError::Missing { path: actual } if actual == path));

        let dir = tempfile::tempdir().unwrap();
        let appeared_path = dir.path().join("appeared.toml");
        fs::write(&appeared_path, "bridge_host = \"external:7600\"\n").unwrap();
        let appeared = LoadedConfigStore {
            path: Some(appeared_path.clone()),
            missing_is_allowed: true,
            original_raw: None,
            config: BhtuneConfig::default(),
            revision: revision_token_for_raw(None),
            toml_allow_uncertain_quality: None,
            toml_tuning: TuningConfig::default(),
            tuning_sources: tuning_config_sources(&TuningConfig::default()),
        };
        let err = save_config_store(
            &appeared,
            &appeared.revision,
            &ConfigPolicyUpdate {
                allow_uncertain_quality: true,
                retention_days: None,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigStoreError::Conflict {
                path: Some(actual), ..
            } if actual == appeared_path
        ));
    }

    #[test]
    fn save_config_store_rejects_unreadable_and_malformed_stored_documents() {
        let dir = tempfile::tempdir().unwrap();
        let unreadable_path = dir.path().join("config-directory");
        fs::create_dir(&unreadable_path).unwrap();
        let unreadable = LoadedConfigStore {
            path: Some(unreadable_path.clone()),
            missing_is_allowed: false,
            original_raw: Some(String::new()),
            config: BhtuneConfig::default(),
            revision: revision_token_for_raw(Some("")),
            toml_allow_uncertain_quality: None,
            toml_tuning: TuningConfig::default(),
            tuning_sources: tuning_config_sources(&TuningConfig::default()),
        };
        let err = save_config_store(
            &unreadable,
            &unreadable.revision,
            &ConfigPolicyUpdate {
                allow_uncertain_quality: true,
                retention_days: None,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigStoreError::Unreadable { path, .. } if path == unreadable_path
        ));

        let malformed_path = dir.path().join("malformed.toml");
        fs::write(&malformed_path, "[").unwrap();
        let malformed = LoadedConfigStore {
            path: Some(malformed_path.clone()),
            missing_is_allowed: false,
            original_raw: Some("[".to_string()),
            config: BhtuneConfig::default(),
            revision: revision_token_for_raw(Some("[")),
            toml_allow_uncertain_quality: None,
            toml_tuning: TuningConfig::default(),
            tuning_sources: tuning_config_sources(&TuningConfig::default()),
        };
        let err = save_config_store(
            &malformed,
            &malformed.revision,
            &ConfigPolicyUpdate {
                allow_uncertain_quality: true,
                retention_days: None,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigStoreError::Malformed {
                path: Some(path), ..
            } if path == malformed_path
        ));
    }

    /// A minimal, `DcsTemplate::validate()`-passing `[[template]]` block: non-empty `name`,
    /// PV/MV suffixes, and every other field either populated or left as an intentionally
    /// empty (not missing) suffix -- see `bhtune_core::template::DcsTemplate::validate`.
    fn valid_templates_toml(name: &str) -> String {
        format!(
            r#"
[[template]]
name = "{name}"
revert_mode = false
proportional_type = "band"
integral_type = "reset_time"
integral_unit = "seconds"
derivative_type = "derivative_time"
derivative_unit = "seconds"
process_variable_suffix = "PV"
manipulated_variable_suffix = "MV"
setpoint_variable_suffix = "SV"
controller_direction_suffix = ""
controller_mode_suffix = ""
mode_attribute_suffix = ""
upper_pv_range_suffix = "SH"
lower_pv_range_suffix = "SL"
upper_mv_range_suffix = "MSH"
lower_mv_range_suffix = "MSL"
proportional_constant_suffix = "P"
integral_constant_suffix = "I"
derivative_constant_suffix = "D"
mode_manual_value = ""
mode_auto_value = ""
controller_action_direct_value = "0"
"#
        )
    }

    #[test]
    fn load_user_templates_nothing_explicit_and_nothing_discovered_returns_none() {
        let templates =
            load_user_templates(None, &BhtuneConfig::default(), None, None, None, false).unwrap();
        assert_eq!(templates, None);
    }

    #[test]
    fn load_user_templates_auto_discovered_path_missing_is_not_an_error() {
        // Points the auto-discovery tier at a real, empty directory that (by construction of
        // a fresh tempdir) has no `bhtune/templates.toml` inside it -- proves a missing file
        // at the *discovered* default is `Ok(None)`, not an error, unlike the explicit-path
        // case below.
        let dir = tempfile::tempdir().unwrap();
        let templates = load_user_templates(
            None,
            &BhtuneConfig::default(),
            Some(dir.path().to_str().unwrap()),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(templates, None);
    }

    #[test]
    fn load_user_templates_explicit_cli_path_missing_is_an_error() {
        let err = load_user_templates(
            Some(PathBuf::from("/nonexistent/templates.toml")),
            &BhtuneConfig::default(),
            None,
            None,
            None,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("templates file not found"));
    }

    #[test]
    fn load_user_templates_explicit_config_key_path_missing_is_an_error() {
        let config = BhtuneConfig {
            templates: Some(PathBuf::from("/nonexistent/templates.toml")),
            ..Default::default()
        };
        let err = load_user_templates(None, &config, None, None, None, false).unwrap_err();
        assert!(err.to_string().contains("templates file not found"));
    }

    #[test]
    fn load_user_templates_valid_file_is_parsed_and_validated() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "{}", valid_templates_toml("Test Template")).unwrap();
        let templates = load_user_templates(
            Some(file.path().to_path_buf()),
            &BhtuneConfig::default(),
            None,
            None,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "Test Template");
    }

    #[test]
    fn load_user_templates_malformed_toml_is_an_error_naming_the_file() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "this is not valid toml [[[").unwrap();
        let err = load_user_templates(
            Some(file.path().to_path_buf()),
            &BhtuneConfig::default(),
            None,
            None,
            None,
            false,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("failed to parse templates file"));
        assert!(message.contains(&file.path().display().to_string()));
    }

    #[test]
    fn load_user_templates_a_template_failing_validation_is_an_error() {
        // `manipulated_variable_suffix` left empty fails `DcsTemplate::validate()` --
        // proves `parse_catalog`'s validation pass (not just its TOML-shape parsing) is
        // surfaced as a hard error too.
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let toml = valid_templates_toml("Broken Template").replace(
            "manipulated_variable_suffix = \"MV\"",
            "manipulated_variable_suffix = \"\"",
        );
        write!(file, "{toml}").unwrap();
        let err = load_user_templates(
            Some(file.path().to_path_buf()),
            &BhtuneConfig::default(),
            None,
            None,
            None,
            false,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("manipulated_variable_suffix must not be empty")
        );
    }

    #[test]
    fn load_user_templates_generic_io_error_is_an_error() {
        // Reading a directory as a file fails with an `IsADirectory`-style error, distinct
        // from `NotFound` -- exercises the catch-all I/O error branch (e.g. permission
        // denied in real usage), mirroring `load_config_file_generic_io_error`.
        let dir = tempfile::tempdir().unwrap();
        let err = load_user_templates(
            Some(dir.path().to_path_buf()),
            &BhtuneConfig::default(),
            None,
            None,
            None,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("failed to read templates file"));
    }

    #[test]
    fn load_user_templates_cli_flag_wins_over_config_key() {
        let mut cli_file = tempfile::NamedTempFile::new().unwrap();
        write!(cli_file, "{}", valid_templates_toml("From CLI Flag")).unwrap();
        let mut config_file = tempfile::NamedTempFile::new().unwrap();
        write!(config_file, "{}", valid_templates_toml("From Config File")).unwrap();

        let config = BhtuneConfig {
            templates: Some(config_file.path().to_path_buf()),
            ..Default::default()
        };
        let templates = load_user_templates(
            Some(cli_file.path().to_path_buf()),
            &config,
            None,
            None,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(templates[0].name, "From CLI Flag");
    }

    #[test]
    fn load_user_templates_config_key_is_used_when_no_cli_flag_is_given() {
        let mut config_file = tempfile::NamedTempFile::new().unwrap();
        write!(config_file, "{}", valid_templates_toml("From Config File")).unwrap();
        let config = BhtuneConfig {
            templates: Some(config_file.path().to_path_buf()),
            ..Default::default()
        };
        let templates = load_user_templates(None, &config, None, None, None, false)
            .unwrap()
            .unwrap();
        assert_eq!(templates[0].name, "From Config File");
    }

    #[test]
    fn resolve_db_path_cli_wins() {
        let config = BhtuneConfig {
            db: Some(PathBuf::from("/config/bhtune.db")),
            ..Default::default()
        };
        let resolved = resolve_db_path(
            Some(PathBuf::from("/cli/bhtune.db")),
            &config,
            Some("/xdg-data"),
            Some("/home/me"),
            None,
            false,
        );
        assert_eq!(resolved, PathBuf::from("/cli/bhtune.db"));
    }

    #[test]
    fn resolve_db_path_config_wins_over_platform_default() {
        let config = BhtuneConfig {
            db: Some(PathBuf::from("/config/bhtune.db")),
            ..Default::default()
        };
        let resolved = resolve_db_path(None, &config, Some("/xdg-data"), None, None, false);
        assert_eq!(resolved, PathBuf::from("/config/bhtune.db"));
    }

    #[test]
    fn resolve_db_path_falls_back_to_platform_default() {
        let resolved = resolve_db_path(
            None,
            &BhtuneConfig::default(),
            Some("/xdg-data"),
            Some("/home/me"),
            None,
            false,
        );
        assert_eq!(resolved, PathBuf::from("/xdg-data/bhtune/bhtune.db"));
    }

    #[test]
    fn resolve_bridge_host_cli_wins() {
        let config = BhtuneConfig {
            bridge_host: Some("configured:1".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_bridge_host(Some("cli:2".to_string()), &config),
            "cli:2".to_string()
        );
    }

    #[test]
    fn resolve_bridge_host_config_wins_over_default() {
        let config = BhtuneConfig {
            bridge_host: Some("configured:1".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_bridge_host(None, &config),
            "configured:1".to_string()
        );
    }

    #[test]
    fn resolve_bridge_host_default() {
        assert_eq!(
            resolve_bridge_host(None, &BhtuneConfig::default()),
            DEFAULT_BRIDGE_HOST.to_string()
        );
    }

    #[test]
    fn resolve_bind_addr_cli_wins() {
        let config = BhtuneConfig {
            bind: Some("0.0.0.0:9999".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_bind_addr(Some("127.0.0.1:1234".to_string()), &config),
            "127.0.0.1:1234".to_string()
        );
    }

    #[test]
    fn resolve_bind_addr_config_wins_over_default() {
        let config = BhtuneConfig {
            bind: Some("0.0.0.0:9999".into()),
            ..Default::default()
        };
        assert_eq!(resolve_bind_addr(None, &config), "0.0.0.0:9999".to_string());
    }

    #[test]
    fn resolve_bind_addr_default() {
        assert_eq!(
            resolve_bind_addr(None, &BhtuneConfig::default()),
            DEFAULT_BIND_ADDR.to_string()
        );
    }

    #[test]
    fn resolve_retention_days_cli_wins() {
        let config = BhtuneConfig {
            retention_days: Some(90),
            ..Default::default()
        };
        assert_eq!(resolve_retention_days(Some(30), &config), Some(30));
    }

    #[test]
    fn resolve_retention_days_config_wins_over_default() {
        let config = BhtuneConfig {
            retention_days: Some(90),
            ..Default::default()
        };
        assert_eq!(resolve_retention_days(None, &config), Some(90));
    }

    #[test]
    fn resolve_retention_days_default_is_retain_forever() {
        // No CLI flag, env var, or config key at all -- the deliberate "ships disabled by
        // default" behavior `history-retention`'s design note calls for, not merely the
        // absence of a hardcoded number.
        assert_eq!(resolve_retention_days(None, &BhtuneConfig::default()), None);
    }

    #[test]
    fn resolve_server_cli_wins() {
        let config = BhtuneConfig {
            server: Some("ConfigServer".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_server(Some("CliServer".to_string()), &config).unwrap(),
            "CliServer"
        );
    }

    #[test]
    fn resolve_server_config_fallback() {
        let config = BhtuneConfig {
            server: Some("ConfigServer".into()),
            ..Default::default()
        };
        assert_eq!(resolve_server(None, &config).unwrap(), "ConfigServer");
    }

    #[test]
    fn resolve_server_neither_set_errors() {
        let err = resolve_server(None, &BhtuneConfig::default()).unwrap_err();
        assert!(err.to_string().contains("no OPC server specified"));
    }

    #[test]
    fn demo_policy_defaults_are_valid_and_safe() {
        let policy = resolve_demo_policy(&DemoPolicyConfig::default()).unwrap();
        assert_eq!(policy, DemoPolicy::default());
        assert!(policy.validate().is_ok());
        assert_eq!(policy.session_ttl_secs, DEMO_SESSION_TTL_SECS);
        assert_eq!(policy.poll_interval_ms, DEMO_POLL_INTERVAL_MS);
        assert_eq!(policy.run_timeout_secs, DEMO_RUN_TIMEOUT_SECS);
        assert_eq!(policy.max_active_runs_global, DEMO_MAX_ACTIVE_RUNS_GLOBAL);
        assert_eq!(
            policy.max_active_runs_per_visitor,
            DEMO_MAX_ACTIVE_RUNS_PER_VISITOR
        );
        assert_eq!(
            policy.accepted_starts_per_token,
            DEMO_ACCEPTED_STARTS_PER_TOKEN
        );
        assert_eq!(
            policy.accepted_starts_per_client_ip,
            DEMO_ACCEPTED_STARTS_PER_CLIENT_IP
        );
        assert_eq!(
            policy.accepted_start_window_secs,
            DEMO_ACCEPTED_START_WINDOW_SECS
        );
        assert_eq!(
            policy.retained_runs_per_visitor,
            DEMO_RETAINED_RUNS_PER_VISITOR
        );
        assert_eq!(
            policy.max_tune_run_rows_global,
            DEMO_MAX_TUNE_RUN_ROWS_GLOBAL
        );
        assert_eq!(policy.max_json_body_bytes, DEMO_MAX_JSON_BODY_BYTES);
        assert_eq!(policy.max_sse_per_visitor, DEMO_MAX_SSE_PER_VISITOR);
        assert_eq!(policy.max_sse_global, DEMO_MAX_SSE_GLOBAL);
        assert_eq!(policy.sse_lifetime_secs, DEMO_SSE_LIFETIME_SECS);
        assert_eq!(
            policy.ordinary_request_concurrency,
            DEMO_ORDINARY_REQUEST_CONCURRENCY
        );
        assert_eq!(
            policy.ordinary_request_timeout_secs,
            DEMO_ORDINARY_REQUEST_TIMEOUT_SECS
        );
        assert_eq!(policy.cleanup_interval_secs, DEMO_CLEANUP_INTERVAL_SECS);
    }

    #[test]
    fn demo_policy_accepts_explicit_contract_values() {
        let explicit = DemoPolicyConfig {
            session_ttl_secs: Some(DEMO_SESSION_TTL_SECS),
            poll_interval_ms: Some(DEMO_POLL_INTERVAL_MS),
            run_timeout_secs: Some(DEMO_RUN_TIMEOUT_SECS),
            max_active_runs_global: Some(DEMO_MAX_ACTIVE_RUNS_GLOBAL),
            max_active_runs_per_visitor: Some(DEMO_MAX_ACTIVE_RUNS_PER_VISITOR),
            accepted_starts_per_token: Some(DEMO_ACCEPTED_STARTS_PER_TOKEN),
            accepted_starts_per_client_ip: Some(DEMO_ACCEPTED_STARTS_PER_CLIENT_IP),
            accepted_start_window_secs: Some(DEMO_ACCEPTED_START_WINDOW_SECS),
            retained_runs_per_visitor: Some(DEMO_RETAINED_RUNS_PER_VISITOR),
            max_runs_per_session: Some(DEMO_MAX_RUNS_PER_SESSION),
            max_tune_run_rows_global: Some(DEMO_MAX_TUNE_RUN_ROWS_GLOBAL),
            max_json_body_bytes: Some(DEMO_MAX_JSON_BODY_BYTES),
            max_sse_per_visitor: Some(DEMO_MAX_SSE_PER_VISITOR),
            max_sse_global: Some(DEMO_MAX_SSE_GLOBAL),
            sse_lifetime_secs: Some(DEMO_SSE_LIFETIME_SECS),
            ordinary_request_concurrency: Some(DEMO_ORDINARY_REQUEST_CONCURRENCY),
            ordinary_request_timeout_secs: Some(DEMO_ORDINARY_REQUEST_TIMEOUT_SECS),
            cleanup_interval_secs: Some(DEMO_CLEANUP_INTERVAL_SECS),
        };
        assert_eq!(
            resolve_demo_policy(&explicit).unwrap(),
            DemoPolicy::default()
        );
    }

    #[test]
    fn demo_policy_rejects_every_contract_override() {
        macro_rules! assert_invalid {
            ($field:ident, $value:expr) => {
                assert!(
                    DemoPolicy {
                        $field: $value,
                        ..DemoPolicy::default()
                    }
                    .validate()
                    .unwrap_err()
                    .contains(concat!("demo.", stringify!($field))),
                    "{} unexpectedly accepted an override",
                    stringify!($field)
                );
            };
        }

        assert_invalid!(session_ttl_secs, DEMO_SESSION_TTL_SECS - 1);
        assert_invalid!(poll_interval_ms, DEMO_POLL_INTERVAL_MS + 1);
        assert_invalid!(run_timeout_secs, DEMO_RUN_TIMEOUT_SECS + 1);
        assert_invalid!(max_active_runs_global, DEMO_MAX_ACTIVE_RUNS_GLOBAL + 1);
        assert_invalid!(
            max_active_runs_per_visitor,
            DEMO_MAX_ACTIVE_RUNS_PER_VISITOR + 1
        );
        assert_invalid!(
            accepted_starts_per_token,
            DEMO_ACCEPTED_STARTS_PER_TOKEN + 1
        );
        assert_invalid!(
            accepted_starts_per_client_ip,
            DEMO_ACCEPTED_STARTS_PER_CLIENT_IP + 1
        );
        assert_invalid!(
            accepted_start_window_secs,
            DEMO_ACCEPTED_START_WINDOW_SECS + 1
        );
        assert_invalid!(
            retained_runs_per_visitor,
            DEMO_RETAINED_RUNS_PER_VISITOR + 1
        );
        assert_invalid!(max_runs_per_session, DEMO_MAX_RUNS_PER_SESSION + 1);
        assert_invalid!(max_tune_run_rows_global, DEMO_MAX_TUNE_RUN_ROWS_GLOBAL + 1);
        assert_invalid!(max_json_body_bytes, DEMO_MAX_JSON_BODY_BYTES + 1);
        assert_invalid!(max_sse_per_visitor, DEMO_MAX_SSE_PER_VISITOR + 1);
        assert_invalid!(max_sse_global, DEMO_MAX_SSE_GLOBAL + 1);
        assert_invalid!(sse_lifetime_secs, DEMO_SSE_LIFETIME_SECS + 1);
        assert_invalid!(
            ordinary_request_concurrency,
            DEMO_ORDINARY_REQUEST_CONCURRENCY + 1
        );
        assert_invalid!(
            ordinary_request_timeout_secs,
            DEMO_ORDINARY_REQUEST_TIMEOUT_SECS + 1
        );
        assert_invalid!(cleanup_interval_secs, DEMO_CLEANUP_INTERVAL_SECS + 1);
    }

    #[test]
    fn demo_policy_config_rejects_an_override_during_resolution() {
        let error = resolve_demo_policy(&DemoPolicyConfig {
            accepted_starts_per_token: Some(DEMO_ACCEPTED_STARTS_PER_TOKEN + 1),
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(
            error,
            format!(
                "demo.accepted_starts_per_token must be exactly \
                 {DEMO_ACCEPTED_STARTS_PER_TOKEN}"
            )
        );
    }

    #[test]
    fn demo_policy_config_rejects_legacy_conflated_rate_keys() {
        let error = toml::from_str::<BhtuneConfig>(
            r#"
[demo]
token_requests_per_window = 64
ip_requests_per_window = 10
rate_window_secs = 10
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn example_config_declares_the_approved_demo_contract() {
        let config: BhtuneConfig = toml::from_str(include_str!("../bhtune.example.toml")).unwrap();
        assert_eq!(config.server_mode, Some(ServerMode::Full));
        assert_eq!(
            resolve_demo_policy_from_config(&config).unwrap(),
            DemoPolicy::default()
        );
    }

    #[cfg(feature = "schemars")]
    #[test]
    fn demo_policy_schema_documents_exact_values_and_distinct_limits() {
        let schema = serde_json::to_value(schemars::schema_for!(DemoPolicyConfig)).unwrap();
        let properties = schema["properties"].as_object().unwrap();
        let expected = [
            ("session_ttl_secs", 86_400_u64),
            ("poll_interval_ms", 200),
            ("run_timeout_secs", 30),
            ("max_active_runs_global", 8),
            ("max_active_runs_per_visitor", 1),
            ("accepted_starts_per_token", 6),
            ("accepted_starts_per_client_ip", 6),
            ("accepted_start_window_secs", 600),
            ("retained_runs_per_visitor", 10),
            ("max_runs_per_session", 10),
            ("max_tune_run_rows_global", 5_000),
            ("max_json_body_bytes", 32_768),
            ("max_sse_per_visitor", 2),
            ("max_sse_global", 32),
            ("sse_lifetime_secs", 45),
            ("ordinary_request_concurrency", 64),
            ("ordinary_request_timeout_secs", 10),
            ("cleanup_interval_secs", 300),
        ];
        assert_eq!(properties.len(), expected.len());
        for (name, value) in expected {
            let property = &properties[name];
            assert_eq!(property["minimum"], value);
            assert_eq!(property["maximum"], value);
            assert!(
                property["description"]
                    .as_str()
                    .is_some_and(|description| !description.is_empty())
            );
        }
        assert_eq!(schema["additionalProperties"], false);
        assert!(!properties.contains_key("token_requests_per_window"));
        assert!(!properties.contains_key("request_timeout_secs"));
    }

    #[test]
    fn server_mode_resolution_prefers_environment_then_config() {
        let config = BhtuneConfig {
            server_mode: Some(ServerMode::Demo),
            ..Default::default()
        };
        assert_eq!(
            resolve_server_mode(Some("full"), &config).unwrap(),
            ServerMode::Full
        );
        assert_eq!(
            resolve_server_mode(None, &config).unwrap(),
            ServerMode::Demo
        );
        let full_config = BhtuneConfig {
            server_mode: Some(ServerMode::Full),
            ..Default::default()
        };
        assert_eq!(
            resolve_server_mode(None, &full_config).unwrap(),
            ServerMode::Full
        );
        assert!(resolve_server_mode(Some("invalid"), &config).is_err());
    }

    #[test]
    fn obsolete_mode_key_no_longer_selects_demo_mode() {
        let config: BhtuneConfig = toml::from_str("mode = \"demo\"").unwrap();
        assert_eq!(config.server_mode, None);
        assert_eq!(
            resolve_server_mode(None, &config).unwrap(),
            ServerMode::Full
        );
    }

    #[test]
    fn demo_origin_requires_https_except_for_explicit_loopback_http() {
        for valid in [
            "https://demo.example",
            "https://demo.example:8443",
            "http://localhost:8787",
            "http://127.0.0.1:8787",
            "http://127.0.0.1:0",
            "http://[::1]:8787",
        ] {
            assert!(validate_demo_origin(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "http://demo.example",
            "http://0.0.0.0:8787",
            "https://demo.example/",
            "https://user@demo.example",
            "https://demo.example/path",
            "https://demo.example?query",
            "https://demo.example#fragment",
            "ftp://demo.example",
            "demo.example",
            " https://demo.example",
            "https://demo.example:65536",
            "https://demo.example:not-a-port",
            "https://[::1",
            "https://::1",
        ] {
            assert!(validate_demo_origin(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn origin_resolution_preserves_full_defaults_and_validates_demo() {
        let config = BhtuneConfig {
            origin: Some("https://config.example".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            resolve_origin(
                Some("https://environment.example".to_owned()),
                &config,
                "127.0.0.1:8787",
                ServerMode::Demo,
            )
            .unwrap(),
            "https://environment.example"
        );
        assert_eq!(
            resolve_origin(None, &config, "127.0.0.1:8787", ServerMode::Demo).unwrap(),
            "https://config.example"
        );
        assert_eq!(
            resolve_origin(
                None,
                &BhtuneConfig::default(),
                "127.0.0.1:8787",
                ServerMode::Demo,
            )
            .unwrap(),
            "http://127.0.0.1:8787"
        );
        assert_eq!(
            resolve_origin(
                None,
                &BhtuneConfig::default(),
                "0.0.0.0:8787",
                ServerMode::Full,
            )
            .unwrap(),
            "http://0.0.0.0:8787"
        );
        assert!(
            resolve_origin(
                None,
                &BhtuneConfig::default(),
                "0.0.0.0:8787",
                ServerMode::Demo,
            )
            .is_err()
        );
    }

    #[test]
    fn demo_trusted_proxy_accepts_only_supported_exact_addresses_and_networks() {
        for valid in [
            None,
            Some("127.0.0.1"),
            Some("::1"),
            Some("10.0.0.0/24"),
            Some("10.0.0.1/24"),
            Some("0.0.0.0/0"),
            Some("192.0.2.4/32"),
            Some("2001:db8::/32"),
        ] {
            assert!(validate_demo_trusted_proxy(valid).is_ok(), "{valid:?}");
        }
        for invalid in [
            Some(""),
            Some(" 10.0.0.1"),
            Some("proxy.example"),
            Some("10.0.0.0/33"),
            Some("::1/129"),
        ] {
            assert!(validate_demo_trusted_proxy(invalid).is_err(), "{invalid:?}");
        }
    }
}
