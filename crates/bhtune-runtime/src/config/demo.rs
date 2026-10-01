#![allow(rustdoc::broken_intra_doc_links)]

use serde::{Deserialize, Serialize};
use std::net::IpAddr;

use super::model::BhtuneConfig;

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

/// Minimum positive process gain accepted by the public Demo.
pub const DEMO_SIM_GAIN_MIN: f32 = 0.1;

/// Maximum positive process gain accepted by the public Demo.
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
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum ServerMode {
    #[default]
    Full,
    Demo,
}

/// Limits applied to the public simulator-only demo surface.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct DemoPolicy {
    /// Anonymous visitor-session lifetime.
    #[cfg_attr(feature = "schemars", schemars(range(min = 86_400, max = 86_400)))]
    pub session_ttl_secs: u64,
    /// Simulator polling interval.
    #[cfg_attr(feature = "schemars", schemars(range(min = 200, max = 200)))]
    pub poll_interval_ms: u64,
    /// Whole-run timeout.
    #[cfg_attr(feature = "schemars", schemars(range(min = 30, max = 30)))]
    pub run_timeout_secs: u64,
    /// Active Demo tune limit across all visitors.
    #[cfg_attr(feature = "schemars", schemars(range(min = 8, max = 8)))]
    pub max_active_runs_global: u32,
    /// Active Demo tune limit for one visitor.
    #[cfg_attr(feature = "schemars", schemars(range(min = 1, max = 1)))]
    pub max_active_runs_per_visitor: u32,
    /// Accepted starts for one session token in the quota window.
    #[cfg_attr(feature = "schemars", schemars(range(min = 6, max = 6)))]
    pub accepted_starts_per_token: u32,
    /// Accepted starts for one client IP in the quota window.
    #[cfg_attr(feature = "schemars", schemars(range(min = 6, max = 6)))]
    pub accepted_starts_per_client_ip: u32,
    /// Window shared by both accepted-start quotas.
    #[cfg_attr(feature = "schemars", schemars(range(min = 600, max = 600)))]
    pub accepted_start_window_secs: u64,
    /// Completed runs retained for one visitor.
    #[cfg_attr(feature = "schemars", schemars(range(min = 10, max = 10)))]
    pub retained_runs_per_visitor: u32,
    /// Maximum total demo runs accepted for one visitor.
    pub max_runs_per_session: u32,
    /// Current Demo-owned `tune_runs` row limit across all visitors.
    #[cfg_attr(feature = "schemars", schemars(range(min = 5_000, max = 5_000)))]
    pub max_tune_run_rows_global: u32,
    /// Maximum JSON request-body size.
    #[cfg_attr(feature = "schemars", schemars(range(min = 32_768, max = 32_768)))]
    pub max_json_body_bytes: u64,
    /// Simultaneous SSE streams for one visitor.
    #[cfg_attr(feature = "schemars", schemars(range(min = 2, max = 2)))]
    pub max_sse_per_visitor: u32,
    /// Simultaneous Demo SSE streams across all visitors.
    #[cfg_attr(feature = "schemars", schemars(range(min = 32, max = 32)))]
    pub max_sse_global: u32,
    /// Absolute lifetime of one Demo SSE stream.
    #[cfg_attr(feature = "schemars", schemars(range(min = 45, max = 45)))]
    pub sse_lifetime_secs: u64,
    /// Concurrent ordinary, non-streaming Demo API requests.
    #[cfg_attr(feature = "schemars", schemars(range(min = 64, max = 64)))]
    pub ordinary_request_concurrency: u32,
    /// Timeout for an ordinary, non-streaming Demo API request.
    #[cfg_attr(feature = "schemars", schemars(range(min = 10, max = 10)))]
    pub ordinary_request_timeout_secs: u64,
    /// Interval between Demo cleanup passes.
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
#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
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
    /// Runs retained for one session. Fixed at 10.
    #[cfg_attr(feature = "schemars", schemars(range(min = 10, max = 10)))]
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

pub fn resolve_origin(
    env_origin: Option<String>,
    config: &BhtuneConfig,
    bind_addr: &str,
    mode: ServerMode,
) -> Result<Option<String>, String> {
    let configured_origin = env_origin.or_else(|| config.origin.clone());
    if mode == ServerMode::Demo {
        let origin = configured_origin.unwrap_or_else(|| format!("http://{bind_addr}"));
        validate_demo_origin(&origin)?;
        return Ok(Some(origin));
    }
    Ok(configured_origin)
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

#[cfg(test)]
mod tests {
    use super::*;
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
            r"
[demo]
token_requests_per_window = 64
ip_requests_per_window = 10
rate_window_secs = 10
",
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
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
    fn origin_resolution_keeps_full_mode_automatic_and_validates_demo() {
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
            Some("https://environment.example".to_owned())
        );
        assert_eq!(
            resolve_origin(None, &config, "127.0.0.1:8787", ServerMode::Demo).unwrap(),
            Some("https://config.example".to_owned())
        );
        assert_eq!(
            resolve_origin(
                None,
                &BhtuneConfig::default(),
                "127.0.0.1:8787",
                ServerMode::Demo,
            )
            .unwrap(),
            Some("http://127.0.0.1:8787".to_owned())
        );
        assert_eq!(
            resolve_origin(
                None,
                &BhtuneConfig::default(),
                "0.0.0.0:8787",
                ServerMode::Full,
            )
            .unwrap(),
            None
        );
        assert_eq!(
            resolve_origin(
                Some("https://configured.example".to_owned()),
                &BhtuneConfig::default(),
                "0.0.0.0:8787",
                ServerMode::Full,
            )
            .unwrap(),
            Some("https://configured.example".to_owned())
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
