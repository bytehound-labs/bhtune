//! Read-only validation of a tune request before the tune execution path is prepared.

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    time::Duration,
};

use bhtune_core::{DcsTemplate, LoopTags, TagOrValue, built_in_templates};
use bhtune_db::{
    SqlitePool,
    models::{DcsTemplateRow, TemplateOrigin},
};
use bhtune_driver::{
    Driver, DriverCapabilities, OpcDaCompatibilityStatus, OpcDaGatewayCompatibility,
    OpcDaGatewayInfo, Quality, ReadOnlyDriver, TagValue, check_gateway_compatibility,
    get_opcda_gateway_info, list_opcda_servers,
};
use serde::Serialize;

use crate::{
    config::{BhtuneConfig, resolve_and_validate_tuning_config},
    driver::build_with_poll_interval,
};

use super::{
    config::{EffectiveTiming, build_loop_config_with_timing, build_loop_tags},
    prepare::{InitialState, validate_initial_state},
    quality::{
        check_quality, parse_f32_value, read_batch_f32, read_batch_raw,
        resolve_direction_from_batch, resolve_f32_from_batch,
    },
    request::{DriverKind, TuneRequest, ValidatedTuneRequest},
};

/// Severity assigned to an individual preflight result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightCheckStatus {
    Pass,
    Warn,
    Fail,
}

impl PreflightCheckStatus {
    /// Returns the stable JSON and table representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

/// One named preflight check and its operator-facing detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreflightCheck {
    pub name: String,
    pub status: PreflightCheckStatus,
    pub detail: String,
}

/// The value, quality, and validation result for one derived OPC tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreflightTagRead {
    pub tag: String,
    pub roles: Vec<String>,
    pub value: Option<String>,
    pub quality: Option<String>,
    pub status: PreflightCheckStatus,
    pub detail: String,
}

/// Read-only results for a tune request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreflightReport {
    pub checks: Vec<PreflightCheck>,
    pub tag_reads: Vec<PreflightTagRead>,
}

impl PreflightReport {
    fn new() -> Self {
        Self {
            checks: Vec::new(),
            tag_reads: Vec::new(),
        }
    }

    fn push_check(
        &mut self,
        name: impl Into<String>,
        status: PreflightCheckStatus,
        detail: impl Into<String>,
    ) {
        self.checks.push(PreflightCheck {
            name: name.into(),
            status,
            detail: detail.into(),
        });
    }

    /// Returns the highest severity present in the report.
    #[must_use]
    pub fn status(&self) -> PreflightCheckStatus {
        if self.has_failures() {
            PreflightCheckStatus::Fail
        } else if self.has_warnings() {
            PreflightCheckStatus::Warn
        } else {
            PreflightCheckStatus::Pass
        }
    }

    /// Returns whether at least one check or tag read failed.
    #[must_use]
    pub fn has_failures(&self) -> bool {
        self.checks
            .iter()
            .any(|check| check.status == PreflightCheckStatus::Fail)
            || self
                .tag_reads
                .iter()
                .any(|tag| tag.status == PreflightCheckStatus::Fail)
    }

    /// Returns whether at least one check or tag read produced a warning.
    #[must_use]
    pub fn has_warnings(&self) -> bool {
        self.checks
            .iter()
            .any(|check| check.status == PreflightCheckStatus::Warn)
            || self
                .tag_reads
                .iter()
                .any(|tag| tag.status == PreflightCheckStatus::Warn)
    }

    /// Returns whether this report passes, including warning policy.
    #[must_use]
    pub fn passes(&self, strict: bool) -> bool {
        !self.has_failures() && (!strict || !self.has_warnings())
    }
}

struct ResolvedTemplate {
    template: DcsTemplate,
    source: &'static str,
}

#[derive(Debug, Clone)]
struct DerivedTag {
    tag: String,
    roles: Vec<String>,
}

/// Checks a request without invoking tune preparation, the polling loop, or any database
/// mutation. An existing database is queried only through a read-only pool; when it is
/// absent, the current built-in and configured catalog templates provide the lookup data.
pub async fn preflight(
    request: ValidatedTuneRequest,
    app_config: &BhtuneConfig,
    database: Option<&SqlitePool>,
    user_templates: Option<&[DcsTemplate]>,
) -> anyhow::Result<PreflightReport> {
    let args = request.into_request();
    let mut report = PreflightReport::new();

    let effective_timing = match resolve_and_validate_tuning_config(
        &app_config.tuning,
        args.driver == DriverKind::Opcda,
    ) {
        Ok(timing) => {
            report.push_check(
                "Configuration and tuning",
                PreflightCheckStatus::Pass,
                format!(
                    "[tuning] is valid: poll={}ms timeout={}s operation={}s restore={}s delay={}s",
                    timing.poll_interval_ms,
                    timing.timeout_secs,
                    timing.op_timeout_secs,
                    timing.restore_timeout_secs,
                    timing.mrft_delay_secs
                ),
            );
            EffectiveTiming::from(timing)
        }
        Err(error) => {
            report.push_check(
                "Configuration and tuning",
                PreflightCheckStatus::Fail,
                error.to_string(),
            );
            return Ok(report);
        }
    };

    let Some(resolved_template) =
        resolve_template(&args.template, database, user_templates).await?
    else {
        report.push_check(
            "Template and tag derivation",
            PreflightCheckStatus::Fail,
            format!("no template named '{}'", args.template),
        );
        return Ok(report);
    };

    let loop_config = match build_loop_config_with_timing(&args, effective_timing) {
        Ok(config) => config,
        Err(error) => {
            report.push_check(
                "Loop configuration",
                PreflightCheckStatus::Fail,
                error.to_string(),
            );
            return Ok(report);
        }
    };

    let tags = match build_loop_tags(&args, &resolved_template.template) {
        Ok(tags) => tags,
        Err(error) => {
            report.push_check(
                "Template and tag derivation",
                PreflightCheckStatus::Fail,
                error.to_string(),
            );
            return Ok(report);
        }
    };
    report.push_check(
        "Template and tag derivation",
        PreflightCheckStatus::Pass,
        format!(
            "resolved '{}' from {}; derived {} tag(s)",
            resolved_template.template.name,
            resolved_template.source,
            derived_tags(&tags).len()
        ),
    );
    report.push_check(
        "Loop configuration",
        PreflightCheckStatus::Pass,
        format!(
            "LoopConfig::validate() passed for {:?}/{:?}",
            loop_config.process_type, loop_config.controller_type
        ),
    );

    let mut args = args;
    args.bridge_host = Some(crate::config::resolve_bridge_host(
        args.bridge_host.take(),
        app_config,
    ));
    let bridge_host = args
        .bridge_host
        .as_deref()
        .unwrap_or(crate::config::DEFAULT_BRIDGE_HOST)
        .to_string();

    if args.driver == DriverKind::Opcda {
        let server = crate::config::resolve_server(args.server.take(), app_config)?;
        args.server = Some(server.clone());
        if !check_gateway_readiness(
            &bridge_host,
            &server,
            effective_timing.op_timeout_secs,
            &mut report,
        )
        .await?
        {
            return Ok(report);
        }
    } else {
        report.push_check(
            "Gateway info",
            PreflightCheckStatus::Pass,
            "not applicable to the in-process simulator",
        );
        report.push_check(
            "Gateway compatibility",
            PreflightCheckStatus::Pass,
            "not applicable to the in-process simulator",
        );
        report.push_check(
            "OPC DA ProgID",
            PreflightCheckStatus::Pass,
            "not applicable to the in-process simulator",
        );
    }

    let driver = within_timeout(
        effective_timing.op_timeout_secs,
        "driver connection",
        async { build_with_poll_interval(&args, effective_timing.poll_interval_ms).await },
    )
    .await?;
    let driver = ReadOnlyDriver::new(driver);

    if args.driver == DriverKind::Opcda {
        match within_timeout(
            effective_timing.op_timeout_secs,
            "server capabilities",
            async { Ok(driver.capabilities().await?) },
        )
        .await
        {
            Ok(capabilities) => report.push_check(
                "Server capabilities",
                PreflightCheckStatus::Pass,
                capabilities_detail(&capabilities),
            ),
            Err(error) => report.push_check(
                "Server capabilities",
                PreflightCheckStatus::Warn,
                format!("server capability discovery is unavailable: {error:#}"),
            ),
        }
    } else {
        report.push_check(
            "Server capabilities",
            PreflightCheckStatus::Pass,
            "not applicable to the in-process simulator",
        );
    }

    let derived = derived_tags(&tags);
    let requested_tags: Vec<String> = derived.iter().map(|tag| tag.tag.clone()).collect();
    let values = match within_timeout(effective_timing.op_timeout_secs, "tag read", async {
        Ok(driver.read(&requested_tags).await?)
    })
    .await
    {
        Ok(values) => values,
        Err(error) => {
            report.push_check(
                "Derived tag reads",
                PreflightCheckStatus::Fail,
                format!("could not read all derived tags: {error:#}"),
            );
            return Ok(report);
        }
    };

    let requested_set: HashSet<&str> = derived.iter().map(|tag| tag.tag.as_str()).collect();
    let mut values_by_tag = HashMap::new();
    let mut duplicate_tags = HashSet::new();
    let mut unexpected_tags = Vec::new();
    for value in values {
        if !requested_set.contains(value.tag.as_str()) {
            unexpected_tags.push(value.tag);
        } else if values_by_tag.contains_key(&value.tag) {
            duplicate_tags.insert(value.tag);
        } else {
            values_by_tag.insert(value.tag.clone(), value);
        }
    }

    report.tag_reads = derived
        .iter()
        .map(|tag| {
            assess_tag_read(
                tag,
                values_by_tag.get(&tag.tag),
                duplicate_tags.contains(&tag.tag),
                app_config.allow_uncertain_quality,
            )
        })
        .collect();

    let tag_read_status = if report
        .tag_reads
        .iter()
        .any(|tag| tag.status == PreflightCheckStatus::Fail)
        || !unexpected_tags.is_empty()
    {
        PreflightCheckStatus::Fail
    } else if report
        .tag_reads
        .iter()
        .any(|tag| tag.status == PreflightCheckStatus::Warn)
    {
        PreflightCheckStatus::Warn
    } else {
        PreflightCheckStatus::Pass
    };
    let mut tag_read_detail = format!(
        "read {} derived tag(s), including value and OPC quality",
        report.tag_reads.len()
    );
    if !unexpected_tags.is_empty() {
        tag_read_detail.push_str(&format!(
            "; driver returned unexpected tag(s): {}",
            unexpected_tags.join(", ")
        ));
    }
    report.push_check("Derived tag reads", tag_read_status, tag_read_detail);

    let (mode_status, mode_detail) = mode_status(
        &tags,
        &resolved_template.template,
        &values_by_tag,
        &report.tag_reads,
    );
    report.push_check("Mode values", mode_status, mode_detail);

    let (mode_attribute_status, mode_attribute_detail) = mode_attribute_status(
        &tags,
        &resolved_template.template,
        &values_by_tag,
        &report.tag_reads,
    );
    report.push_check(
        "Mode attribute",
        mode_attribute_status,
        mode_attribute_detail,
    );

    match initial_state_from_batch(
        &driver,
        &tags,
        &resolved_template.template,
        &values_by_tag,
        app_config.allow_uncertain_quality,
    )
    .await
    {
        Ok(initial) => match validate_initial_state(&initial) {
            Ok(()) => report.push_check(
                "Initial state",
                PreflightCheckStatus::Pass,
                "PV/MV values are numeric and the PV/MV ranges are valid; initial MV is in range",
            ),
            Err(error) => report.push_check(
                "Initial state",
                PreflightCheckStatus::Fail,
                error.to_string(),
            ),
        },
        Err(error) => report.push_check(
            "Initial state",
            PreflightCheckStatus::Fail,
            error.to_string(),
        ),
    }

    report.push_check(
        "Write-back readiness",
        write_back_status(&args, &tags, &report.tag_reads, mode_attribute_status),
        write_back_detail(&args, &tags, &report.tag_reads, mode_attribute_status),
    );

    Ok(report)
}

async fn check_gateway_readiness(
    bridge_host: &str,
    server: &str,
    op_timeout_secs: u64,
    report: &mut PreflightReport,
) -> anyhow::Result<bool> {
    let gateway_info = within_timeout(op_timeout_secs, "gateway info", async {
        Ok(get_opcda_gateway_info(bridge_host).await?)
    })
    .await;
    match &gateway_info {
        Ok(info) => report.push_check(
            "Gateway info",
            PreflightCheckStatus::Pass,
            gateway_info_detail(info),
        ),
        Err(error) => report.push_check(
            "Gateway info",
            PreflightCheckStatus::Warn,
            format!("gateway-wide metadata is unavailable: {error:#}"),
        ),
    }

    let compatibility = within_timeout(op_timeout_secs, "gateway compatibility", async {
        Ok(check_gateway_compatibility(bridge_host, Some(server)).await?)
    })
    .await?;
    report.push_check(
        "Gateway compatibility",
        compatibility_status(&compatibility),
        compatibility_detail(&compatibility),
    );

    let servers = within_timeout(op_timeout_secs, "OPC DA server discovery", async {
        Ok(list_opcda_servers(bridge_host).await?)
    })
    .await?;
    let server_found = servers
        .iter()
        .any(|registered| registered.eq_ignore_ascii_case(server));
    report.push_check(
        "OPC DA ProgID",
        if server_found {
            PreflightCheckStatus::Pass
        } else {
            PreflightCheckStatus::Fail
        },
        if server_found {
            format!("'{server}' is registered on the gateway")
        } else if servers.is_empty() {
            format!("'{server}' is not registered; the gateway reported no servers")
        } else {
            format!(
                "'{server}' is not registered; available servers: {}",
                servers.join(", ")
            )
        },
    );

    Ok(server_found && !compatibility.is_incompatible())
}

async fn resolve_template(
    name: &str,
    database: Option<&SqlitePool>,
    user_templates: Option<&[DcsTemplate]>,
) -> anyhow::Result<Option<ResolvedTemplate>> {
    let builtins = built_in_templates();
    let builtin = builtins.iter().find(|template| template.name == name);
    let catalog = user_templates
        .and_then(|templates| templates.iter().find(|template| template.name == name));

    if let Some(row) = match database {
        Some(pool) => DcsTemplateRow::get_by_name(pool, name).await?,
        None => None,
    } {
        let (template, source) = match row.origin {
            TemplateOrigin::Builtin => (
                builtin.cloned().unwrap_or(row.template),
                "built-in template",
            ),
            TemplateOrigin::Catalog => (
                catalog.cloned().unwrap_or(row.template),
                "user catalog template",
            ),
            TemplateOrigin::User => (row.template, "database user template"),
        };
        return Ok(Some(ResolvedTemplate { template, source }));
    }

    Ok(builtin
        .cloned()
        .map(|template| ResolvedTemplate {
            template,
            source: "built-in template",
        })
        .or_else(|| {
            catalog.cloned().map(|template| ResolvedTemplate {
                template,
                source: "user catalog template",
            })
        }))
}

fn derived_tags(tags: &LoopTags) -> Vec<DerivedTag> {
    let mut derived: Vec<DerivedTag> = Vec::new();
    let mut indices: HashMap<String, usize> = HashMap::new();

    let mut add = |tag: &str, role: &str| {
        if let Some(index) = indices.get(tag).copied() {
            let roles = &mut derived[index].roles;
            if !roles.iter().any(|existing| existing == role) {
                roles.push(role.to_string());
            }
        } else {
            indices.insert(tag.to_string(), derived.len());
            derived.push(DerivedTag {
                tag: tag.to_string(),
                roles: vec![role.to_string()],
            });
        }
    };

    add(&tags.process_variable, "process_variable");
    add(&tags.manipulated_variable, "manipulated_variable");
    if let Some(tag) = &tags.setpoint_variable {
        add(tag, "setpoint_variable");
    }
    if let Some(tag) = &tags.controller_mode {
        add(tag, "controller_mode");
    }
    if let Some(tag) = &tags.mode_attribute {
        add(tag, "mode_attribute");
    }
    if let TagOrValue::Tag(tag) = &tags.upper_pv_range {
        add(tag, "upper_pv_range");
    }
    if let TagOrValue::Tag(tag) = &tags.lower_pv_range {
        add(tag, "lower_pv_range");
    }
    if let TagOrValue::Tag(tag) = &tags.upper_mv_range {
        add(tag, "upper_mv_range");
    }
    if let TagOrValue::Tag(tag) = &tags.lower_mv_range {
        add(tag, "lower_mv_range");
    }
    if let TagOrValue::Tag(tag) = &tags.controller_direction {
        add(tag, "controller_direction");
    }
    if let Some(tag) = &tags.proportional_constant {
        add(tag, "proportional_constant");
    }
    if let Some(tag) = &tags.integral_constant {
        add(tag, "integral_constant");
    }
    if let Some(tag) = &tags.derivative_constant {
        add(tag, "derivative_constant");
    }
    derived
}

fn assess_tag_read(
    derived: &DerivedTag,
    value: Option<&TagValue>,
    duplicate: bool,
    allow_uncertain: bool,
) -> PreflightTagRead {
    let Some(value) = value else {
        return PreflightTagRead {
            tag: derived.tag.clone(),
            roles: derived.roles.clone(),
            value: None,
            quality: None,
            status: PreflightCheckStatus::Fail,
            detail: "driver returned no value for this tag".to_string(),
        };
    };

    let mut status = if duplicate {
        PreflightCheckStatus::Fail
    } else {
        PreflightCheckStatus::Pass
    };
    let mut details = vec![format!("quality {:?}", value.quality)];
    if duplicate {
        details.push("driver returned this tag more than once".to_string());
    }

    match check_quality(&derived.tag, value.quality, allow_uncertain) {
        Ok(()) if value.quality == Quality::Uncertain => {
            status = highest_status(status, PreflightCheckStatus::Warn);
            details.push("Uncertain quality is accepted by configuration".to_string());
        }
        Ok(()) => {}
        Err(error) => {
            status = PreflightCheckStatus::Fail;
            details.push(error.to_string());
        }
    }

    if status != PreflightCheckStatus::Fail || value.quality == Quality::Uncertain {
        for role in &derived.roles {
            if is_numeric_role(role)
                && let Err(error) = parse_f32_value(&derived.tag, &value.value)
            {
                status = PreflightCheckStatus::Fail;
                details.push(error.to_string());
                break;
            }
        }
    }

    PreflightTagRead {
        tag: derived.tag.clone(),
        roles: derived.roles.clone(),
        value: Some(value.value.clone()),
        quality: Some(format!("{:?}", value.quality)),
        status,
        detail: details.join("; "),
    }
}

fn is_numeric_role(role: &str) -> bool {
    matches!(
        role,
        "process_variable"
            | "manipulated_variable"
            | "setpoint_variable"
            | "upper_pv_range"
            | "lower_pv_range"
            | "upper_mv_range"
            | "lower_mv_range"
            | "proportional_constant"
            | "integral_constant"
            | "derivative_constant"
    )
}

fn mode_status(
    tags: &LoopTags,
    template: &DcsTemplate,
    values: &HashMap<String, TagValue>,
    tag_reads: &[PreflightTagRead],
) -> (PreflightCheckStatus, String) {
    let Some(mode_tag) = &tags.controller_mode else {
        return (
            PreflightCheckStatus::Pass,
            "template does not define a controller mode tag".to_string(),
        );
    };

    let Some(read) = tag_reads.iter().find(|read| read.tag == *mode_tag) else {
        return (
            PreflightCheckStatus::Fail,
            format!("mode tag '{mode_tag}' was not included in the read results"),
        );
    };
    if read.status == PreflightCheckStatus::Fail {
        return (
            PreflightCheckStatus::Fail,
            format!("mode tag '{mode_tag}' did not have a usable value and quality"),
        );
    }

    let Some(value) = values.get(mode_tag) else {
        return (
            PreflightCheckStatus::Fail,
            format!("driver returned no value for mode tag '{mode_tag}'"),
        );
    };
    if value.value == template.mode_manual_value || value.value == template.mode_auto_value {
        (
            read.status,
            format!(
                "mode tag '{mode_tag}' value '{}' matches the template's Manual or Auto value",
                value.value
            ),
        )
    } else {
        (
            PreflightCheckStatus::Fail,
            format!(
                "mode tag '{mode_tag}' value '{}' matches neither template value ('{}' or '{}')",
                value.value, template.mode_manual_value, template.mode_auto_value
            ),
        )
    }
}

fn mode_attribute_status(
    tags: &LoopTags,
    template: &DcsTemplate,
    values: &HashMap<String, TagValue>,
    tag_reads: &[PreflightTagRead],
) -> (PreflightCheckStatus, String) {
    let Some(attribute_tag) = &tags.mode_attribute else {
        return (
            PreflightCheckStatus::Pass,
            "template does not define a mode-attribute tag".to_string(),
        );
    };
    let Some(program_value) = &template.mode_attribute_program_value else {
        return (
            PreflightCheckStatus::Fail,
            format!("mode-attribute tag '{attribute_tag}' has no template program value"),
        );
    };
    let Some(read) = tag_reads.iter().find(|read| read.tag == *attribute_tag) else {
        return (
            PreflightCheckStatus::Fail,
            format!("mode-attribute tag '{attribute_tag}' was not included in the read results"),
        );
    };
    if read.status == PreflightCheckStatus::Fail {
        return (
            PreflightCheckStatus::Fail,
            format!("mode-attribute tag '{attribute_tag}' did not have a usable value and quality"),
        );
    }
    let Some(value) = values.get(attribute_tag) else {
        return (
            PreflightCheckStatus::Fail,
            format!("driver returned no value for mode-attribute tag '{attribute_tag}'"),
        );
    };
    if value.value == *program_value {
        (
            read.status,
            format!(
                "mode-attribute tag '{attribute_tag}' value matches the template's program value"
            ),
        )
    } else {
        (
            PreflightCheckStatus::Warn,
            format!(
                "mode-attribute tag '{attribute_tag}' value '{}' differs from the template's \
                 program value '{program_value}'",
                value.value
            ),
        )
    }
}

async fn initial_state_from_batch(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    values: &HashMap<String, TagValue>,
    allow_uncertain: bool,
) -> anyhow::Result<InitialState> {
    let pv_ini = read_batch_f32(values, &tags.process_variable, allow_uncertain)?;
    let mv_ini = read_batch_f32(values, &tags.manipulated_variable, allow_uncertain)?;
    let mode_raw = match &tags.controller_mode {
        Some(tag) => Some(read_batch_raw(values, tag, allow_uncertain)?),
        None => None,
    };
    let mode_attribute_raw = match &tags.mode_attribute {
        Some(tag) => Some(read_batch_raw(values, tag, allow_uncertain)?),
        None => None,
    };
    let setpoint_ini = match (&tags.setpoint_variable, &mode_raw) {
        (Some(tag), Some(mode)) if mode == &template.mode_auto_value => {
            Some(read_batch_f32(values, tag, allow_uncertain)?)
        }
        _ => None,
    };
    let direction = resolve_direction_from_batch(
        driver,
        values,
        &tags.controller_direction,
        template,
        allow_uncertain,
    )
    .await?;
    let pv_range_high =
        resolve_f32_from_batch(driver, values, &tags.upper_pv_range, allow_uncertain).await?;
    let pv_range_low =
        resolve_f32_from_batch(driver, values, &tags.lower_pv_range, allow_uncertain).await?;
    let mv_range_high =
        resolve_f32_from_batch(driver, values, &tags.upper_mv_range, allow_uncertain).await?;
    let mv_range_low =
        resolve_f32_from_batch(driver, values, &tags.lower_mv_range, allow_uncertain).await?;

    Ok(InitialState {
        pv_ini,
        mv_ini,
        pv_range_high,
        pv_range_low,
        mv_range_high,
        mv_range_low,
        direction,
        mode_raw,
        mode_attribute_raw,
        setpoint_ini,
    })
}

fn pid_constant_tags(tags: &LoopTags) -> [(&'static str, Option<&str>); 3] {
    [
        ("Proportional", tags.proportional_constant.as_deref()),
        ("Integral", tags.integral_constant.as_deref()),
        ("Derivative", tags.derivative_constant.as_deref()),
    ]
}

fn write_back_status(
    args: &TuneRequest,
    tags: &LoopTags,
    reads: &[PreflightTagRead],
    mode_attribute_status: PreflightCheckStatus,
) -> PreflightCheckStatus {
    if args.driver == DriverKind::Simulator {
        return if args.write_pid.is_some() {
            PreflightCheckStatus::Fail
        } else {
            PreflightCheckStatus::Pass
        };
    }

    let required = pid_constant_tags(tags);
    let missing: Vec<&str> = required
        .iter()
        .filter_map(|(label, tag)| tag.is_none().then_some(*label))
        .collect();
    if !missing.is_empty() {
        return if args.write_pid.is_some() {
            PreflightCheckStatus::Fail
        } else {
            PreflightCheckStatus::Warn
        };
    }

    if mode_attribute_status == PreflightCheckStatus::Fail
        || (args.write_pid.is_some() && mode_attribute_status == PreflightCheckStatus::Warn)
    {
        return PreflightCheckStatus::Fail;
    }

    let statuses: Vec<PreflightCheckStatus> = required
        .iter()
        .filter_map(|(_, tag)| *tag)
        .filter_map(|tag| reads.iter().find(|read| read.tag == tag))
        .map(|read| read.status)
        .collect();
    if statuses.len() != required.len() || statuses.contains(&PreflightCheckStatus::Fail) {
        PreflightCheckStatus::Fail
    } else if statuses.contains(&PreflightCheckStatus::Warn)
        || mode_attribute_status == PreflightCheckStatus::Warn
    {
        PreflightCheckStatus::Warn
    } else {
        PreflightCheckStatus::Pass
    }
}

fn write_back_detail(
    args: &TuneRequest,
    tags: &LoopTags,
    reads: &[PreflightTagRead],
    mode_attribute_status: PreflightCheckStatus,
) -> String {
    if args.driver == DriverKind::Simulator {
        return if args.write_pid.is_some() {
            "PID write-back is not available for the simulator".to_string()
        } else {
            "not applicable: the simulator has no PID constant tags".to_string()
        };
    }

    let required = pid_constant_tags(tags);
    let missing: Vec<&str> = required
        .iter()
        .filter_map(|(label, tag)| tag.is_none().then_some(*label))
        .collect();
    if !missing.is_empty() {
        return format!(
            "{} PID constant tag(s) are not configured{}",
            missing.join(", "),
            if args.write_pid.is_some() {
                "; the requested write-back cannot be prepared"
            } else {
                "; write-back would be skipped"
            }
        );
    }

    if mode_attribute_status == PreflightCheckStatus::Fail
        || (args.write_pid.is_some() && mode_attribute_status == PreflightCheckStatus::Warn)
    {
        return if args.write_pid.is_some() {
            "the mode attribute is not confirmed at the template's program value; the \
             requested write-back is not ready"
                .to_string()
        } else {
            "mode-attribute readiness could not be confirmed".to_string()
        };
    }

    let unreadable: Vec<&str> = required
        .iter()
        .filter_map(|(label, tag)| {
            let tag = (*tag)?;
            reads
                .iter()
                .find(|read| read.tag == tag)
                .filter(|read| read.status != PreflightCheckStatus::Pass)
                .map(|_| *label)
        })
        .collect();
    if !unreadable.is_empty() {
        return format!(
            "{} PID constant tag(s) have failed or warning-level reads{}",
            unreadable.join(", "),
            if args.write_pid.is_some() {
                "; the requested write-back is not ready"
            } else {
                "; write-back readiness is incomplete"
            }
        );
    }

    let mut detail = "P/I/D tags are derived and readable as numeric values; this read-only \
                      check does not probe controller write permissions"
        .to_string();
    if mode_attribute_status == PreflightCheckStatus::Warn {
        detail.push_str("; current mode attribute differs from the template's program value");
    }
    detail
}

fn compatibility_status(compatibility: &OpcDaGatewayCompatibility) -> PreflightCheckStatus {
    match compatibility.status {
        OpcDaCompatibilityStatus::Full => PreflightCheckStatus::Pass,
        OpcDaCompatibilityStatus::Partial | OpcDaCompatibilityStatus::Unknown => {
            PreflightCheckStatus::Warn
        }
        OpcDaCompatibilityStatus::Incompatible => PreflightCheckStatus::Fail,
    }
}

fn compatibility_detail(compatibility: &OpcDaGatewayCompatibility) -> String {
    match compatibility.status {
        OpcDaCompatibilityStatus::Full => {
            format!(
                "gateway protocol is fully compatible ({})",
                compatibility.client_version
            )
        }
        OpcDaCompatibilityStatus::Partial | OpcDaCompatibilityStatus::Unknown => {
            compatibility_warning_detail(compatibility, compatibility.warning_message())
        }
        OpcDaCompatibilityStatus::Incompatible => compatibility.incompatibility_message(),
    }
}

fn compatibility_warning_detail(
    compatibility: &OpcDaGatewayCompatibility,
    warning: Option<String>,
) -> String {
    warning.unwrap_or_else(|| format!("compatibility status is {}", compatibility.status.as_str()))
}

fn gateway_info_detail(info: &OpcDaGatewayInfo) -> String {
    let features = info
        .features
        .iter()
        .map(|feature| {
            format!(
                "{} {}-{}",
                feature.feature.as_str(),
                feature.min_version,
                feature.max_version
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "gateway {} (compatibility schema {}); features: {}",
        info.application_version, info.compatibility_schema_version, features
    )
}

fn capabilities_detail(capabilities: &DriverCapabilities) -> String {
    format!(
        "server {} protocol {}; browse sessions={}, live search={}, indexed search={}",
        capabilities.application_version,
        capabilities.protocol_version,
        capabilities.supports_browse_sessions,
        capabilities.supports_search,
        capabilities.supports_indexed_search
    )
}

fn highest_status(
    current: PreflightCheckStatus,
    candidate: PreflightCheckStatus,
) -> PreflightCheckStatus {
    match (current, candidate) {
        (PreflightCheckStatus::Fail, _) | (_, PreflightCheckStatus::Fail) => {
            PreflightCheckStatus::Fail
        }
        (PreflightCheckStatus::Warn, _) | (_, PreflightCheckStatus::Warn) => {
            PreflightCheckStatus::Warn
        }
        _ => PreflightCheckStatus::Pass,
    }
}

async fn within_timeout<T>(
    timeout_secs: u64,
    operation: &str,
    future: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    tokio::time::timeout(Duration::from_secs(timeout_secs), future)
        .await
        .map_err(|_| anyhow::anyhow!("{operation} timed out after {timeout_secs}s"))?
}

#[cfg(test)]
mod tests;
