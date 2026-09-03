use axum::http::{HeaderMap, header};
use axum::{Json, Router, extract::State};
use bhtune_cli::config::{
    DEMO_COOKIE_NAME, DEMO_CYCLES_COUNT_DEFAULT, DEMO_CYCLES_COUNT_MAX, DEMO_CYCLES_COUNT_MIN,
    DEMO_CYCLES_SKIP_DEFAULT, DEMO_CYCLES_SKIP_MAX, DEMO_CYCLES_SKIP_MIN,
    DEMO_NOISE_PROTECTION_SECS_DEFAULT, DEMO_NOISE_PROTECTION_SECS_MAX,
    DEMO_NOISE_PROTECTION_SECS_MIN, DEMO_POLL_INTERVAL_MS, DEMO_RANGE_ENDPOINT_MAX,
    DEMO_RANGE_ENDPOINT_MIN, DEMO_RANGE_HIGH, DEMO_RANGE_LOW, DEMO_RANGE_SPAN_MAX,
    DEMO_RANGE_SPAN_MIN, DEMO_RELAY_AMP_DEFAULT, DEMO_RELAY_AMP_MAX, DEMO_RELAY_AMP_MIN,
    DEMO_RUN_TIMEOUT_SECS, DEMO_SIM_DEAD_TIME_DEFAULT, DEMO_SIM_DEAD_TIME_MAX,
    DEMO_SIM_DEAD_TIME_MIN, DEMO_SIM_GAIN_ABS_MIN, DEMO_SIM_GAIN_DEFAULT, DEMO_SIM_GAIN_MAX,
    DEMO_SIM_INITIAL_VALUE_DEFAULT, DEMO_SIM_NOISE_DEFAULT, DEMO_SIM_NOISE_MAX_PV_SPAN_FRACTION,
    DEMO_SIM_SEED_DEFAULT, DEMO_SIM_SEED_MAX, DEMO_SIM_TAU_DEFAULT, DEMO_SIM_TAU_MAX,
    DEMO_SIM_TAU_MIN, DEMO_TAG_NAME, DEMO_TEMPLATE_NAME, DemoPolicy, ServerMode,
};
use bhtune_core::{ControllerDirection, ControllerType, ProcessType, built_in_templates};
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
pub struct CapabilityActions {
    pub start_simulator_tune: bool,
    pub start_opcda_tune: bool,
    pub cancel_run: bool,
    pub stream_run: bool,
    pub list_history: bool,
    pub export_run: bool,
    pub delete_run: bool,
    pub edit_notes: bool,
    pub write_pid: bool,
    pub revert_pid: bool,
    pub manage_templates: bool,
    pub manage_config: bool,
    pub browse_opc: bool,
}

impl CapabilityActions {
    fn for_mode(mode: ServerMode) -> Self {
        let demo = mode == ServerMode::Demo;
        Self {
            start_simulator_tune: true,
            start_opcda_tune: !demo,
            cancel_run: true,
            stream_run: true,
            list_history: true,
            export_run: true,
            delete_run: true,
            edit_notes: !demo,
            write_pid: !demo,
            revert_pid: !demo,
            manage_templates: !demo,
            manage_config: !demo,
            browse_opc: !demo,
        }
    }
}

#[derive(Debug, Serialize, ToSchema, PartialEq)]
pub struct FloatBounds {
    pub min: f32,
    pub max: f32,
    /// When present, values strictly between `-absolute_min` and `absolute_min` are invalid.
    pub absolute_min: Option<f32>,
}

#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
pub struct IntegerBounds {
    pub min: u64,
    pub max: u64,
}

#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
pub struct ProcessControllerCompatibility {
    pub process_type: ProcessType,
    pub controller_types: Vec<ControllerType>,
}

#[derive(Debug, Serialize, ToSchema, PartialEq)]
pub struct DemoSimulatorDefaults {
    pub tag_name: String,
    pub template: String,
    pub direction: ControllerDirection,
    pub pv_range: FloatBounds,
    pub mv_range: FloatBounds,
    pub poll_interval_ms: u64,
    pub run_timeout_secs: u64,
    pub relay_amp: f32,
    pub cycles_skip: u32,
    pub cycles_count: u32,
    pub noise_protection_secs: u32,
    pub sim_gain: f32,
    pub sim_tau: f32,
    pub sim_dead_time: f32,
    pub sim_noise: f32,
    pub sim_seed: u64,
    pub sim_initial_pv: f32,
    pub sim_initial_mv: f32,
}

#[derive(Debug, Serialize, ToSchema, PartialEq)]
pub struct DemoSimulatorLimits {
    pub relay_amp: FloatBounds,
    pub cycles_skip: IntegerBounds,
    pub cycles_count: IntegerBounds,
    pub noise_protection_secs: IntegerBounds,
    pub sim_gain: FloatBounds,
    pub sim_tau: FloatBounds,
    pub sim_dead_time: FloatBounds,
    pub sim_seed: IntegerBounds,
    pub range_endpoint: FloatBounds,
    pub range_span: FloatBounds,
    pub max_noise_fraction_of_pv_span: f32,
}

#[derive(Debug, Serialize, ToSchema, PartialEq)]
pub struct DemoSimulatorCapabilities {
    pub template: String,
    pub templates: Vec<String>,
    pub tag_name: String,
    pub process_types: Vec<ProcessType>,
    pub controller_types: Vec<ControllerType>,
    pub compatibility: Vec<ProcessControllerCompatibility>,
    pub defaults: DemoSimulatorDefaults,
    pub limits: DemoSimulatorLimits,
}

#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
pub struct DemoRestrictions {
    pub simulator_only: bool,
    pub built_in_templates_only: bool,
    pub fixed_tag_name: bool,
    pub direction_must_match_process_gain: bool,
    pub custom_tag_mappings_allowed: bool,
    pub notes_allowed: bool,
    pub automatic_pid_write_allowed: bool,
    pub post_run_pid_write_allowed: bool,
}

#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
pub struct DemoQuotas {
    pub max_active_runs_global: u32,
    pub max_active_runs_per_visitor: u32,
    pub max_runs_per_session: u32,
    pub accepted_starts_per_token: u32,
    pub accepted_starts_per_client_ip: u32,
    pub accepted_start_window_secs: u64,
    pub retained_runs_per_visitor: u32,
    pub max_tune_run_rows_global: u32,
    pub max_json_body_bytes: u64,
    pub max_sse_per_visitor: u32,
    pub max_sse_global: u32,
    pub sse_lifetime_secs: u64,
    pub ordinary_request_concurrency: u32,
    pub ordinary_request_timeout_secs: u64,
}

impl From<DemoPolicy> for DemoQuotas {
    fn from(policy: DemoPolicy) -> Self {
        Self {
            max_active_runs_global: policy.max_active_runs_global,
            max_active_runs_per_visitor: policy.max_active_runs_per_visitor,
            max_runs_per_session: policy.max_runs_per_session,
            accepted_starts_per_token: policy.accepted_starts_per_token,
            accepted_starts_per_client_ip: policy.accepted_starts_per_client_ip,
            accepted_start_window_secs: policy.accepted_start_window_secs,
            retained_runs_per_visitor: policy.retained_runs_per_visitor,
            max_tune_run_rows_global: policy.max_tune_run_rows_global,
            max_json_body_bytes: policy.max_json_body_bytes,
            max_sse_per_visitor: policy.max_sse_per_visitor,
            max_sse_global: policy.max_sse_global,
            sse_lifetime_secs: policy.sse_lifetime_secs,
            ordinary_request_concurrency: policy.ordinary_request_concurrency,
            ordinary_request_timeout_secs: policy.ordinary_request_timeout_secs,
        }
    }
}

#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
pub struct CookieCapabilities {
    pub name: String,
    pub path: String,
    pub max_age_secs: u64,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: String,
}

#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
pub struct SecurityCapabilities {
    pub allowed_origin: String,
    pub exact_origin_required_for_mutations: bool,
    pub https_required: bool,
    pub loopback_http_allowed: bool,
    pub trusted_proxy_configured: bool,
    pub forwarded_client_ip_header: Option<String>,
    pub cookie: Option<CookieCapabilities>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CapabilitiesResponse {
    pub mode: ServerMode,
    /// Driver identifiers accepted by the mode's tune-start surface.
    pub drivers: Vec<String>,
    pub actions: CapabilityActions,
    pub demo: bool,
    pub demo_policy: Option<DemoPolicy>,
    pub simulator: Option<DemoSimulatorCapabilities>,
    pub restrictions: Option<DemoRestrictions>,
    pub quotas: Option<DemoQuotas>,
    pub security: SecurityCapabilities,
}

fn compatibility() -> Vec<ProcessControllerCompatibility> {
    ProcessType::ALL
        .into_iter()
        .map(|process_type| ProcessControllerCompatibility {
            process_type,
            controller_types: ControllerType::ALL
                .into_iter()
                .filter(|controller_type| controller_type.is_allowed_for(process_type))
                .collect(),
        })
        .collect()
}

fn demo_simulator() -> DemoSimulatorCapabilities {
    let process_types = ProcessType::ALL.to_vec();
    DemoSimulatorCapabilities {
        template: DEMO_TEMPLATE_NAME.to_owned(),
        tag_name: DEMO_TAG_NAME.to_owned(),
        controller_types: ControllerType::ALL.to_vec(),
        compatibility: compatibility(),
        defaults: DemoSimulatorDefaults {
            tag_name: DEMO_TAG_NAME.to_owned(),
            template: DEMO_TEMPLATE_NAME.to_owned(),
            direction: ControllerDirection::Reverse,
            pv_range: FloatBounds {
                min: DEMO_RANGE_LOW,
                max: DEMO_RANGE_HIGH,
                absolute_min: None,
            },
            mv_range: FloatBounds {
                min: DEMO_RANGE_LOW,
                max: DEMO_RANGE_HIGH,
                absolute_min: None,
            },
            poll_interval_ms: DEMO_POLL_INTERVAL_MS,
            run_timeout_secs: DEMO_RUN_TIMEOUT_SECS,
            relay_amp: DEMO_RELAY_AMP_DEFAULT,
            cycles_skip: DEMO_CYCLES_SKIP_DEFAULT,
            cycles_count: DEMO_CYCLES_COUNT_DEFAULT,
            noise_protection_secs: DEMO_NOISE_PROTECTION_SECS_DEFAULT,
            sim_gain: DEMO_SIM_GAIN_DEFAULT,
            sim_tau: DEMO_SIM_TAU_DEFAULT,
            sim_dead_time: DEMO_SIM_DEAD_TIME_DEFAULT,
            sim_noise: DEMO_SIM_NOISE_DEFAULT,
            sim_seed: DEMO_SIM_SEED_DEFAULT,
            sim_initial_pv: DEMO_SIM_INITIAL_VALUE_DEFAULT,
            sim_initial_mv: DEMO_SIM_INITIAL_VALUE_DEFAULT,
        },
        limits: DemoSimulatorLimits {
            relay_amp: FloatBounds {
                min: DEMO_RELAY_AMP_MIN,
                max: DEMO_RELAY_AMP_MAX,
                absolute_min: None,
            },
            cycles_skip: IntegerBounds {
                min: u64::from(DEMO_CYCLES_SKIP_MIN),
                max: u64::from(DEMO_CYCLES_SKIP_MAX),
            },
            cycles_count: IntegerBounds {
                min: u64::from(DEMO_CYCLES_COUNT_MIN),
                max: u64::from(DEMO_CYCLES_COUNT_MAX),
            },
            noise_protection_secs: IntegerBounds {
                min: u64::from(DEMO_NOISE_PROTECTION_SECS_MIN),
                max: u64::from(DEMO_NOISE_PROTECTION_SECS_MAX),
            },
            sim_gain: FloatBounds {
                min: -DEMO_SIM_GAIN_MAX,
                max: DEMO_SIM_GAIN_MAX,
                absolute_min: Some(DEMO_SIM_GAIN_ABS_MIN),
            },
            sim_tau: FloatBounds {
                min: DEMO_SIM_TAU_MIN,
                max: DEMO_SIM_TAU_MAX,
                absolute_min: None,
            },
            sim_dead_time: FloatBounds {
                min: DEMO_SIM_DEAD_TIME_MIN,
                max: DEMO_SIM_DEAD_TIME_MAX,
                absolute_min: None,
            },
            sim_seed: IntegerBounds {
                min: 0,
                max: DEMO_SIM_SEED_MAX,
            },
            range_endpoint: FloatBounds {
                min: DEMO_RANGE_ENDPOINT_MIN,
                max: DEMO_RANGE_ENDPOINT_MAX,
                absolute_min: None,
            },
            range_span: FloatBounds {
                min: DEMO_RANGE_SPAN_MIN,
                max: DEMO_RANGE_SPAN_MAX,
                absolute_min: None,
            },
            max_noise_fraction_of_pv_span: DEMO_SIM_NOISE_MAX_PV_SPAN_FRACTION,
        },
        templates: built_in_templates()
            .into_iter()
            .map(|template| template.name)
            .collect(),
        process_types,
    }
}

#[utoipa::path(
    get,
    path = "/api/capabilities",
    tag = "health",
    responses((status = 200, body = CapabilitiesResponse))
)]
pub(crate) async fn capabilities(
    State(state): State<AppState>,
    request_headers: HeaderMap,
) -> Result<(HeaderMap, Json<CapabilitiesResponse>), ApiError> {
    let demo = state.mode == ServerMode::Demo;
    let _request_permit = if demo {
        Some(crate::routes::demo::ordinary_request_permit(&state)?)
    } else {
        None
    };
    let actions = CapabilityActions::for_mode(state.mode);
    let mut response_headers = HeaderMap::new();
    if demo
        && let Some(cookie) =
            crate::routes::demo::session_cookie_header(&request_headers, state.demo_policy)?
    {
        response_headers.insert(header::SET_COOKIE, cookie);
    }
    Ok((
        response_headers,
        Json(CapabilitiesResponse {
            mode: state.mode,
            drivers: if demo {
                vec!["simulator".to_owned()]
            } else {
                vec!["opcda".to_owned(), "simulator".to_owned()]
            },
            actions,
            demo,
            demo_policy: demo.then_some(state.demo_policy),
            simulator: demo.then(demo_simulator),
            restrictions: demo.then_some(DemoRestrictions {
                simulator_only: true,
                built_in_templates_only: true,
                fixed_tag_name: true,
                direction_must_match_process_gain: true,
                custom_tag_mappings_allowed: false,
                notes_allowed: false,
                automatic_pid_write_allowed: false,
                post_run_pid_write_allowed: false,
            }),
            quotas: demo.then(|| state.demo_policy.into()),
            security: SecurityCapabilities {
                allowed_origin: state.allowed_origin.clone().unwrap_or_default(),
                exact_origin_required_for_mutations: demo,
                https_required: demo,
                loopback_http_allowed: demo,
                trusted_proxy_configured: state.trusted_proxy.is_some(),
                forwarded_client_ip_header: state
                    .trusted_proxy
                    .is_some()
                    .then(|| "X-BHTune-Client-IP".to_owned()),
                cookie: demo.then_some(CookieCapabilities {
                    name: DEMO_COOKIE_NAME.to_owned(),
                    path: "/".to_owned(),
                    max_age_secs: state.demo_policy.session_ttl_secs,
                    http_only: true,
                    secure: true,
                    same_site: "Strict".to_owned(),
                }),
            },
        }),
    ))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/capabilities", axum::routing::get(capabilities))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn demo_capabilities_publish_the_authoritative_contract() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        state.allowed_origin = Some("https://demo.test".to_owned());
        state.trusted_proxy = Some("127.0.0.1".to_owned());

        let (headers, Json(response)) = capabilities(State(state.clone()), HeaderMap::new())
            .await
            .unwrap();

        assert!(response.demo);
        assert_eq!(response.drivers, ["simulator"]);
        assert_eq!(response.demo_policy, Some(DemoPolicy::default()));
        assert!(response.actions.start_simulator_tune);
        assert!(!response.actions.start_opcda_tune);
        assert!(!response.actions.write_pid);
        assert_eq!(
            response
                .simulator
                .as_ref()
                .unwrap()
                .compatibility
                .iter()
                .find(|item| item.process_type == ProcessType::Flow)
                .unwrap()
                .controller_types,
            [ControllerType::P, ControllerType::Pi]
        );
        assert_eq!(
            response.simulator.as_ref().unwrap().limits.cycles_count,
            IntegerBounds { min: 1, max: 3 }
        );
        assert_eq!(
            response.simulator.as_ref().unwrap().limits.cycles_skip,
            IntegerBounds { min: 0, max: 2 }
        );
        assert_eq!(
            response.simulator.as_ref().unwrap().limits.sim_gain,
            FloatBounds {
                min: -5.0,
                max: 5.0,
                absolute_min: Some(0.1),
            }
        );
        assert_eq!(
            response.simulator.as_ref().unwrap().defaults.relay_amp,
            10.0
        );
        assert_eq!(
            response.simulator.as_ref().unwrap().defaults.cycles_count,
            2
        );
        assert_eq!(response.simulator.as_ref().unwrap().defaults.sim_tau, 0.5);
        assert_eq!(
            response.simulator.as_ref().unwrap().defaults.sim_dead_time,
            1.0
        );
        assert_eq!(
            response.simulator.as_ref().unwrap().defaults.direction,
            ControllerDirection::Reverse
        );
        assert_eq!(
            response.simulator.as_ref().unwrap().tag_name,
            "Simulator demo"
        );
        assert_eq!(
            response.simulator.as_ref().unwrap().templates.len(),
            built_in_templates().len()
        );
        assert_eq!(
            response.security.cookie.as_ref().unwrap().name,
            DEMO_COOKIE_NAME
        );
        assert_eq!(response.security.allowed_origin, "https://demo.test");
        assert!(response.security.trusted_proxy_configured);
        assert_eq!(
            response.security.forwarded_client_ip_header.as_deref(),
            Some("X-BHTune-Client-IP")
        );
        assert_eq!(
            response.quotas.as_ref().unwrap().retained_runs_per_visitor,
            DemoPolicy::default().retained_runs_per_visitor
        );
        let cookie = headers[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.starts_with("__Host-bhtune_demo_session="));
        assert!(cookie.contains("; Path=/; Max-Age=86400; HttpOnly; SameSite=Strict; Secure"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM demo_sessions")
                .fetch_one(&state.pool)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn full_capabilities_preserve_the_complete_action_surface() {
        let state = crate::test_support::in_memory_state().await;

        let (headers, Json(response)) = capabilities(State(state), HeaderMap::new()).await.unwrap();

        assert!(!response.demo);
        assert_eq!(response.drivers, ["opcda", "simulator"]);
        assert!(response.actions.start_opcda_tune);
        assert!(response.actions.write_pid);
        assert!(response.actions.manage_config);
        assert_eq!(response.demo_policy, None);
        assert!(response.simulator.is_none());
        assert!(response.restrictions.is_none());
        assert!(response.quotas.is_none());
        assert!(response.security.cookie.is_none());
        assert!(!response.security.exact_origin_required_for_mutations);
        assert!(!headers.contains_key(header::SET_COOKIE));
    }

    #[tokio::test]
    async fn demo_capabilities_preserve_an_existing_valid_cookie() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("other=value; {DEMO_COOKIE_NAME}={}", "ab".repeat(32))
                .parse()
                .unwrap(),
        );

        let (response_headers, _) = capabilities(State(state), headers).await.unwrap();

        assert!(!response_headers.contains_key(header::SET_COOKIE));
    }

    #[tokio::test]
    async fn demo_capabilities_replace_a_malformed_cookie() {
        let mut state = crate::test_support::in_memory_state().await;
        state.mode = ServerMode::Demo;
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{DEMO_COOKIE_NAME}=invalid").parse().unwrap(),
        );

        let (response_headers, _) = capabilities(State(state), headers).await.unwrap();

        assert!(response_headers.contains_key(header::SET_COOKIE));
    }
}
