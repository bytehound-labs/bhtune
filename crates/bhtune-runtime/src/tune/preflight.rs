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
        let op_timeout_secs = effective_timing.op_timeout_secs;

        let gateway_info = within_timeout(op_timeout_secs, "gateway info", async {
            Ok(get_opcda_gateway_info(&bridge_host).await?)
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
            Ok(check_gateway_compatibility(&bridge_host, Some(&server)).await?)
        })
        .await?;
        report.push_check(
            "Gateway compatibility",
            compatibility_status(&compatibility),
            compatibility_detail(&compatibility),
        );

        let servers = within_timeout(op_timeout_secs, "OPC DA server discovery", async {
            Ok(list_opcda_servers(&bridge_host).await?)
        })
        .await?;
        let server_found = servers
            .iter()
            .any(|registered| registered.eq_ignore_ascii_case(&server));
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

        if !server_found || compatibility.is_incompatible() {
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
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::test_support::{MockBridgeService, start_mock_server};
    use bhtune_core::{ControllerDirection, ControllerType, ProcessType, ResponseLevel};
    use opcda_bridge_proto::bridge::{
        GetGatewayInfoResponse, ListServersResponse, ProtocolFeature, ProtocolFeatureKind,
        ReadResponse, TagValue as ProtoTagValue,
    };
    use tonic::Status;

    #[test]
    fn preflight_status_strings_are_stable() {
        assert_eq!(PreflightCheckStatus::Pass.as_str(), "pass");
        assert_eq!(PreflightCheckStatus::Warn.as_str(), "warn");
        assert_eq!(PreflightCheckStatus::Fail.as_str(), "fail");
    }

    #[test]
    fn highest_status_preserves_the_most_severe_result() {
        assert_eq!(
            highest_status(PreflightCheckStatus::Fail, PreflightCheckStatus::Pass),
            PreflightCheckStatus::Fail
        );
        assert_eq!(
            highest_status(PreflightCheckStatus::Pass, PreflightCheckStatus::Warn),
            PreflightCheckStatus::Warn
        );
        assert_eq!(
            highest_status(PreflightCheckStatus::Pass, PreflightCheckStatus::Pass),
            PreflightCheckStatus::Pass
        );
    }

    #[test]
    fn compatibility_warning_detail_has_a_status_fallback() {
        let compatibility = OpcDaGatewayCompatibility::unverified();
        assert_eq!(
            compatibility_warning_detail(&compatibility, None),
            format!("compatibility status is {}", compatibility.status.as_str())
        );
    }

    fn protocol_feature(
        kind: ProtocolFeatureKind,
        min_version: u32,
        max_version: u32,
    ) -> ProtocolFeature {
        ProtocolFeature {
            kind: kind as i32,
            min_version,
            max_version,
        }
    }

    fn full_gateway_info() -> GetGatewayInfoResponse {
        GetGatewayInfoResponse {
            application_version: "0.5.9".into(),
            compatibility_schema_version: 1,
            features: vec![
                protocol_feature(ProtocolFeatureKind::Core, 1, 1),
                protocol_feature(ProtocolFeatureKind::Namespace, 2, 3),
                protocol_feature(ProtocolFeatureKind::IndexedSearch, 2, 2),
            ],
        }
    }

    fn test_request(driver: DriverKind, write_pid: Option<ResponseLevel>) -> ValidatedTuneRequest {
        let simulator = driver == DriverKind::Simulator;
        ValidatedTuneRequest::try_from(TuneRequest {
            tagname: "Area1.LIC101.PV".to_string(),
            template: "Yokogawa CentumVP".to_string(),
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp: 10.0,
            cycles_skip: None,
            cycles_count: Some(2),
            noise_protection_secs: Some(0),
            driver,
            bridge_host: None,
            server: Some("Matrikon.OPC.Simulation.1".to_string()),
            sim_gain: 1.0,
            sim_tau: 2.0,
            sim_dead_time: 5.0,
            sim_noise: 0.0,
            sim_seed: 0,
            sim_initial_pv: 50.0,
            sim_initial_mv: 50.0,
            pv_range_high: simulator.then_some(100.0),
            pv_range_low: simulator.then_some(0.0),
            mv_range_high: simulator.then_some(100.0),
            mv_range_low: simulator.then_some(0.0),
            direction: simulator.then_some(ControllerDirection::Reverse),
            tag_overrides: None,
            notes: None,
            yes: false,
            write_pid,
            #[cfg(test)]
            mrft_delay: 0,
            #[cfg(test)]
            poll_interval_ms: 800,
            #[cfg(test)]
            timeout_secs: 3600,
            #[cfg(test)]
            op_timeout_secs: 30,
            #[cfg(test)]
            restore_timeout_secs: 30,
        })
        .unwrap()
    }

    fn test_request_for_template(
        driver: DriverKind,
        template: &str,
        write_pid: Option<ResponseLevel>,
    ) -> ValidatedTuneRequest {
        let mut request = test_request(driver, write_pid).into_request();
        request.template = template.to_string();
        ValidatedTuneRequest::try_from(request).unwrap()
    }

    fn read_response(template: &DcsTemplate) -> ReadResponse {
        let tags = LoopTags::derive_from_pv_tag("Area1.LIC101.PV", template);
        let derived = derived_tags(&tags);
        ReadResponse {
            values: derived
                .into_iter()
                .map(|tag| {
                    let value =
                        if tag.roles.iter().any(|role| {
                            role == "process_variable" || role == "manipulated_variable"
                        }) {
                            "50".to_string()
                        } else if tag.roles.iter().any(|role| role == "controller_mode") {
                            template.mode_manual_value.clone()
                        } else if tag.roles.iter().any(|role| role == "mode_attribute") {
                            template
                                .mode_attribute_program_value
                                .clone()
                                .unwrap_or_else(|| "1".to_string())
                        } else if tag.roles.iter().any(|role| role == "controller_direction") {
                            template.controller_action_direct_value.clone()
                        } else if tag
                            .roles
                            .iter()
                            .any(|role| role == "upper_pv_range" || role == "upper_mv_range")
                        {
                            "100".to_string()
                        } else if tag
                            .roles
                            .iter()
                            .any(|role| role == "lower_pv_range" || role == "lower_mv_range")
                        {
                            "0".to_string()
                        } else {
                            "1".to_string()
                        };
                    ProtoTagValue {
                        tag_id: tag.tag,
                        value,
                        quality: "Good".to_string(),
                        timestamp: "2026-01-01 00:00:00".to_string(),
                    }
                })
                .collect(),
        }
    }

    async fn run_with_mock(
        service: MockBridgeService,
        request: ValidatedTuneRequest,
        config: &BhtuneConfig,
    ) -> (PreflightReport, MockBridgeService) {
        let templates = built_in_templates();
        run_with_mock_templates(service, request, config, &templates).await
    }

    async fn run_with_mock_templates(
        service: MockBridgeService,
        request: ValidatedTuneRequest,
        config: &BhtuneConfig,
        templates: &[DcsTemplate],
    ) -> (PreflightReport, MockBridgeService) {
        let observed = service.clone();
        let (host, server) = start_mock_server(service).await;
        let mut config = config.clone();
        config.bridge_host = Some(host);
        let report = preflight(request, &config, None, Some(templates)).await;
        server.shutdown().await;
        (report.unwrap(), observed)
    }

    fn full_mock_service() -> MockBridgeService {
        let template = built_in_templates()
            .into_iter()
            .find(|template| template.name == "Yokogawa CentumVP")
            .unwrap();
        mock_service_for_template(&template)
    }

    fn mock_service_for_template(template: &DcsTemplate) -> MockBridgeService {
        MockBridgeService {
            gateway_info_response: full_gateway_info(),
            list_servers_response: ListServersResponse {
                servers: vec!["Matrikon.OPC.Simulation.1".into()],
            },
            read_response: read_response(template),
            ..Default::default()
        }
    }

    fn find_check<'a>(report: &'a PreflightReport, name: &str) -> &'a PreflightCheck {
        report
            .checks
            .iter()
            .find(|check| check.name == name)
            .unwrap()
    }

    #[tokio::test]
    async fn preflight_passes_with_compatible_gateway_and_readable_tags_without_yes() {
        let request = test_request(DriverKind::Opcda, Some(ResponseLevel::Moderate));
        let template = built_in_templates()
            .into_iter()
            .find(|template| template.name == "Yokogawa CentumVP")
            .unwrap();
        let expected_tags =
            derived_tags(&build_loop_tags(request.as_request(), &template).unwrap());
        let (report, service) =
            run_with_mock(full_mock_service(), request, &BhtuneConfig::default()).await;

        assert_eq!(report.status(), PreflightCheckStatus::Pass);
        assert!(report.passes(false));
        assert_eq!(report.tag_reads.len(), expected_tags.len());
        assert!(
            expected_tags
                .iter()
                .all(|expected| report.tag_reads.iter().any(|read| read.tag == expected.tag))
        );
        assert!(
            report
                .tag_reads
                .iter()
                .all(|read| read.value.is_some() && read.quality.as_deref() == Some("Good"))
        );
        assert!(
            find_check(&report, "Write-back readiness")
                .detail
                .contains("does not probe controller write permissions")
        );
        assert_eq!(service.read_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn preflight_fails_when_the_prog_id_is_not_registered() {
        let mut service = full_mock_service();
        service.list_servers_response = ListServersResponse { servers: vec![] };
        let (report, service) = run_with_mock(
            service,
            test_request(DriverKind::Opcda, None),
            &BhtuneConfig::default(),
        )
        .await;

        assert_eq!(report.status(), PreflightCheckStatus::Fail);
        assert!(report.has_failures());
        assert_eq!(service.read_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn preflight_reports_nonmatching_registered_servers() {
        let mut service = full_mock_service();
        service.list_servers_response = ListServersResponse {
            servers: vec!["Different.Server".into()],
        };
        let (report, service) = run_with_mock(
            service,
            test_request(DriverKind::Opcda, None),
            &BhtuneConfig::default(),
        )
        .await;

        assert_eq!(
            find_check(&report, "OPC DA ProgID").status,
            PreflightCheckStatus::Fail
        );
        assert!(
            find_check(&report, "OPC DA ProgID")
                .detail
                .contains("Different.Server")
        );
        assert_eq!(service.read_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn preflight_requires_a_server_for_opc_da() {
        let mut request = test_request(DriverKind::Opcda, None).into_request();
        request.server = None;
        let request = ValidatedTuneRequest::try_from(request).unwrap();
        let error = preflight(request, &BhtuneConfig::default(), None, None)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("no OPC server specified"));
    }

    #[tokio::test]
    async fn preflight_refuses_incompatible_gateway_before_tag_reads() {
        let mut service = full_mock_service();
        service.gateway_info_response = GetGatewayInfoResponse {
            application_version: "0.1.0".into(),
            compatibility_schema_version: 1,
            features: vec![protocol_feature(ProtocolFeatureKind::Core, 9, 9)],
        };
        let (report, service) = run_with_mock(
            service,
            test_request(DriverKind::Opcda, None),
            &BhtuneConfig::default(),
        )
        .await;

        assert_eq!(report.status(), PreflightCheckStatus::Fail);
        assert_eq!(service.read_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn preflight_reports_uncertain_quality_as_a_warning_when_allowed() {
        let mut service = full_mock_service();
        service.read_response.values[0].quality = "Uncertain".to_string();
        let (report, _) = run_with_mock(
            service,
            test_request(DriverKind::Opcda, None),
            &BhtuneConfig::default(),
        )
        .await;

        assert_eq!(report.status(), PreflightCheckStatus::Warn);
        assert!(report.passes(false));
        assert!(!report.passes(true));
    }

    #[tokio::test]
    async fn preflight_fails_when_uncertain_quality_is_not_allowed() {
        let mut service = full_mock_service();
        service.read_response.values[0].quality = "Uncertain".to_string();
        let mut config = BhtuneConfig {
            allow_uncertain_quality: false,
            ..Default::default()
        };
        let templates = built_in_templates();
        let observed = service.clone();
        let (host, server) = start_mock_server(service).await;
        config.bridge_host = Some(host);
        let report = preflight(
            test_request(DriverKind::Opcda, None),
            &config,
            None,
            Some(&templates),
        )
        .await
        .unwrap();
        server.shutdown().await;

        assert_eq!(report.status(), PreflightCheckStatus::Fail);
        assert_eq!(observed.read_calls.load(Ordering::SeqCst), 1);
        assert!(
            report
                .tag_reads
                .iter()
                .any(|read| read.quality.as_deref() == Some("Uncertain")
                    && read.status == PreflightCheckStatus::Fail)
        );
    }

    #[tokio::test]
    async fn preflight_warns_when_gateway_info_is_unavailable_but_legacy_compatibility_works() {
        let mut service = full_mock_service();
        service.gateway_info_error = Some(Status::unimplemented("gateway info unavailable"));
        let (report, _) = run_with_mock(
            service,
            test_request(DriverKind::Opcda, None),
            &BhtuneConfig::default(),
        )
        .await;

        assert_eq!(
            find_check(&report, "Gateway info").status,
            PreflightCheckStatus::Warn
        );
        assert_eq!(
            find_check(&report, "Server capabilities").status,
            PreflightCheckStatus::Pass
        );
        assert!(report.passes(false));
        assert!(!report.passes(true));
    }

    #[tokio::test]
    async fn preflight_warns_when_server_capabilities_are_unavailable() {
        let mut service = full_mock_service();
        service.capabilities_error = Some(Status::unimplemented("capabilities unavailable"));
        let (report, _) = run_with_mock(
            service,
            test_request(DriverKind::Opcda, None),
            &BhtuneConfig::default(),
        )
        .await;

        assert_eq!(
            find_check(&report, "Server capabilities").status,
            PreflightCheckStatus::Warn
        );
        assert_eq!(
            find_check(&report, "Gateway info").status,
            PreflightCheckStatus::Pass
        );
        assert_eq!(report.status(), PreflightCheckStatus::Warn);
    }

    #[tokio::test]
    async fn preflight_warns_for_partial_gateway_compatibility() {
        let mut service = full_mock_service();
        service.gateway_info_response = GetGatewayInfoResponse {
            application_version: "0.5.9".into(),
            compatibility_schema_version: 1,
            features: vec![protocol_feature(ProtocolFeatureKind::Core, 1, 1)],
        };
        let (report, _) = run_with_mock(
            service,
            test_request(DriverKind::Opcda, None),
            &BhtuneConfig::default(),
        )
        .await;

        assert_eq!(
            find_check(&report, "Gateway compatibility").status,
            PreflightCheckStatus::Warn
        );
        assert_eq!(report.status(), PreflightCheckStatus::Warn);
    }

    #[tokio::test]
    async fn preflight_reports_failed_tag_reads_without_attempting_any_write() {
        let mut service = full_mock_service();
        service.read_error = Some(Status::unavailable("read unavailable"));
        let (report, service) = run_with_mock(
            service,
            test_request(DriverKind::Opcda, None),
            &BhtuneConfig::default(),
        )
        .await;

        assert_eq!(
            find_check(&report, "Derived tag reads").status,
            PreflightCheckStatus::Fail
        );
        assert!(report.tag_reads.is_empty());
        assert_eq!(service.read_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn preflight_fails_bad_quality_and_invalid_initial_state() {
        let mut service = full_mock_service();
        let template = built_in_templates()
            .into_iter()
            .find(|template| template.name == "Yokogawa CentumVP")
            .unwrap();
        let tags = LoopTags::derive_from_pv_tag("Area1.LIC101.PV", &template);
        service.read_response = ReadResponse {
            values: derived_tags(&tags)
                .into_iter()
                .map(|tag| ProtoTagValue {
                    tag_id: tag.tag.clone(),
                    value: if tag.roles.iter().any(|role| role == "manipulated_variable") {
                        "101".to_string()
                    } else if tag.roles.iter().any(|role| role == "upper_pv_range") {
                        "100".to_string()
                    } else if tag.roles.iter().any(|role| role == "lower_pv_range") {
                        "0".to_string()
                    } else {
                        "50".to_string()
                    },
                    quality: if tag.roles.iter().any(|role| role == "process_variable") {
                        "Bad".to_string()
                    } else {
                        "Good".to_string()
                    },
                    timestamp: "2026-01-01 00:00:00".to_string(),
                })
                .collect(),
        };
        let (report, _) = run_with_mock(
            service,
            test_request(DriverKind::Opcda, None),
            &BhtuneConfig::default(),
        )
        .await;

        assert_eq!(report.status(), PreflightCheckStatus::Fail);
        assert!(report.checks.iter().any(|check| {
            check.name == "Initial state" && check.status == PreflightCheckStatus::Fail
        }));
        assert!(report.tag_reads.iter().any(|read| {
            read.quality.as_deref() == Some("Bad") && read.status == PreflightCheckStatus::Fail
        }));
    }

    #[tokio::test]
    async fn preflight_fails_when_the_initial_mv_is_outside_the_configured_range() {
        let mut service = full_mock_service();
        let template = built_in_templates()
            .into_iter()
            .find(|template| template.name == "Yokogawa CentumVP")
            .unwrap();
        let tags = LoopTags::derive_from_pv_tag("Area1.LIC101.PV", &template);
        let manipulated_variable = tags.manipulated_variable;
        service
            .read_response
            .values
            .iter_mut()
            .find(|value| value.tag_id == manipulated_variable)
            .unwrap()
            .value = "101".to_string();

        let (report, _) = run_with_mock(
            service,
            test_request(DriverKind::Opcda, None),
            &BhtuneConfig::default(),
        )
        .await;

        let initial_state = find_check(&report, "Initial state");
        assert_eq!(initial_state.status, PreflightCheckStatus::Fail);
        assert!(initial_state.detail.contains("initial MV"));
    }

    #[tokio::test]
    async fn simulator_preflight_does_not_contact_a_gateway_or_require_pid_tags() {
        let request = test_request(DriverKind::Simulator, None);
        let report = preflight(
            request,
            &BhtuneConfig::default(),
            None,
            Some(&built_in_templates()),
        )
        .await
        .unwrap();

        assert_eq!(report.status(), PreflightCheckStatus::Pass);
        assert!(report.checks.iter().all(|check| {
            check.name != "Gateway compatibility" || check.status == PreflightCheckStatus::Pass
        }));
    }

    #[tokio::test]
    async fn preflight_reports_an_unknown_template_before_driver_creation() {
        let request = test_request_for_template(DriverKind::Simulator, "Missing Template", None);
        let templates = built_in_templates();
        let report = preflight(request, &BhtuneConfig::default(), None, Some(&templates))
            .await
            .unwrap();

        assert_eq!(report.status(), PreflightCheckStatus::Fail);
        assert!(report.tag_reads.is_empty());
        assert!(
            find_check(&report, "Template and tag derivation")
                .detail
                .contains("no template named")
        );
    }

    #[tokio::test]
    async fn preflight_reports_invalid_loop_configuration_before_driver_creation() {
        let mut request = test_request(DriverKind::Simulator, None).into_request();
        request.relay_amp = 0.0;
        let request = ValidatedTuneRequest::try_from(request).unwrap();
        let templates = built_in_templates();
        let report = preflight(request, &BhtuneConfig::default(), None, Some(&templates))
            .await
            .unwrap();

        assert_eq!(report.status(), PreflightCheckStatus::Fail);
        assert!(report.tag_reads.is_empty());
        assert_eq!(
            find_check(&report, "Loop configuration").status,
            PreflightCheckStatus::Fail
        );
    }

    #[tokio::test]
    async fn preflight_reports_simulator_tag_derivation_errors_before_driver_creation() {
        let mut request = test_request(DriverKind::Simulator, None).into_request();
        request.pv_range_high = None;
        let request = ValidatedTuneRequest::try_from(request).unwrap();
        let templates = built_in_templates();
        let report = preflight(request, &BhtuneConfig::default(), None, Some(&templates))
            .await
            .unwrap();

        assert_eq!(report.status(), PreflightCheckStatus::Fail);
        assert!(report.tag_reads.is_empty());
        assert_eq!(
            find_check(&report, "Template and tag derivation").status,
            PreflightCheckStatus::Fail
        );
    }

    #[tokio::test]
    async fn preflight_checks_auto_setpoint_and_mode_values_against_the_template() {
        let mut service = full_mock_service();
        let template = built_in_templates()
            .into_iter()
            .find(|template| template.name == "Yokogawa CentumVP")
            .unwrap();
        let tags = LoopTags::derive_from_pv_tag("Area1.LIC101.PV", &template);
        let mode_tag = tags.controller_mode.as_deref().unwrap();
        service
            .read_response
            .values
            .iter_mut()
            .find(|value| value.tag_id == mode_tag)
            .unwrap()
            .value = template.mode_auto_value.clone();
        let (report, _) = run_with_mock(
            service,
            test_request(DriverKind::Opcda, None),
            &BhtuneConfig::default(),
        )
        .await;

        assert_eq!(report.status(), PreflightCheckStatus::Pass);
        assert!(
            find_check(&report, "Mode values")
                .detail
                .contains("matches the template")
        );
        assert_eq!(
            find_check(&report, "Mode attribute").status,
            PreflightCheckStatus::Pass
        );
        assert_eq!(
            find_check(&report, "Initial state").status,
            PreflightCheckStatus::Pass
        );
    }

    #[tokio::test]
    async fn preflight_fails_mode_values_that_do_not_match_the_template() {
        let mut service = full_mock_service();
        let template = built_in_templates()
            .into_iter()
            .find(|template| template.name == "Yokogawa CentumVP")
            .unwrap();
        let tags = LoopTags::derive_from_pv_tag("Area1.LIC101.PV", &template);
        let mode_tag = tags.controller_mode.as_deref().unwrap();
        service
            .read_response
            .values
            .iter_mut()
            .find(|value| value.tag_id == mode_tag)
            .unwrap()
            .value = "CAS".to_string();
        let (report, _) = run_with_mock(
            service,
            test_request(DriverKind::Opcda, None),
            &BhtuneConfig::default(),
        )
        .await;

        assert_eq!(
            find_check(&report, "Mode values").status,
            PreflightCheckStatus::Fail
        );
        assert_eq!(report.status(), PreflightCheckStatus::Fail);
    }

    #[tokio::test]
    async fn preflight_checks_honeywell_mode_attribute_against_program_value() {
        let template = built_in_templates()
            .into_iter()
            .find(|template| template.name == "Honeywell Experion")
            .unwrap();
        let service = mock_service_for_template(&template);
        let (report, _) = run_with_mock_templates(
            service,
            test_request_for_template(
                DriverKind::Opcda,
                "Honeywell Experion",
                Some(ResponseLevel::Moderate),
            ),
            &BhtuneConfig::default(),
            std::slice::from_ref(&template),
        )
        .await;

        assert_eq!(
            find_check(&report, "Mode attribute").status,
            PreflightCheckStatus::Pass
        );
        assert_eq!(
            find_check(&report, "Write-back readiness").status,
            PreflightCheckStatus::Pass
        );
    }

    #[tokio::test]
    async fn preflight_rejects_requested_writeback_when_mode_attribute_is_not_program() {
        let template = built_in_templates()
            .into_iter()
            .find(|template| template.name == "Honeywell Experion")
            .unwrap();
        let mut service = mock_service_for_template(&template);
        let tags = LoopTags::derive_from_pv_tag("Area1.LIC101.PV", &template);
        let mode_attribute_tag = tags.mode_attribute.as_deref().unwrap();
        service
            .read_response
            .values
            .iter_mut()
            .find(|value| value.tag_id == mode_attribute_tag)
            .unwrap()
            .value = "1".to_string();
        let (report, _) = run_with_mock_templates(
            service,
            test_request_for_template(
                DriverKind::Opcda,
                "Honeywell Experion",
                Some(ResponseLevel::Moderate),
            ),
            &BhtuneConfig::default(),
            std::slice::from_ref(&template),
        )
        .await;

        assert_eq!(
            find_check(&report, "Mode attribute").status,
            PreflightCheckStatus::Warn
        );
        assert_eq!(
            find_check(&report, "Write-back readiness").status,
            PreflightCheckStatus::Fail
        );
        assert_eq!(report.status(), PreflightCheckStatus::Fail);
    }

    #[tokio::test]
    async fn preflight_reports_missing_duplicate_and_unexpected_read_values() {
        let mut service = full_mock_service();
        let removed_tag = service.read_response.values[0].tag_id.clone();
        let duplicated_value = service.read_response.values[1].clone();
        service.read_response.values.remove(0);
        service.read_response.values.push(duplicated_value);
        service.read_response.values.push(ProtoTagValue {
            tag_id: "Unexpected.Tag".to_string(),
            value: "1".to_string(),
            quality: "Good".to_string(),
            timestamp: "2026-01-01 00:00:00".to_string(),
        });
        let (report, _) = run_with_mock(
            service,
            test_request(DriverKind::Opcda, None),
            &BhtuneConfig::default(),
        )
        .await;

        assert_eq!(report.status(), PreflightCheckStatus::Fail);
        assert_eq!(
            report
                .tag_reads
                .iter()
                .find(|read| read.tag == removed_tag)
                .unwrap()
                .status,
            PreflightCheckStatus::Fail
        );
        assert!(
            report
                .checks
                .iter()
                .find(|check| check.name == "Derived tag reads")
                .unwrap()
                .detail
                .contains("Unexpected.Tag")
        );
    }

    #[test]
    fn derived_tags_merge_duplicate_roles_and_keep_one_read_target() {
        let template = built_in_templates().remove(0);
        let mut tags = LoopTags::derive_from_pv_tag("Area1.LIC101.PV", &template);
        tags.manipulated_variable = tags.process_variable.clone();
        let derived = derived_tags(&tags);
        let merged = derived
            .iter()
            .find(|tag| tag.tag == tags.process_variable)
            .unwrap();

        assert_eq!(
            derived
                .iter()
                .filter(|tag| tag.tag == tags.process_variable)
                .count(),
            1
        );
        assert!(merged.roles.iter().any(|role| role == "process_variable"));
        assert!(
            merged
                .roles
                .iter()
                .any(|role| role == "manipulated_variable")
        );
    }

    #[test]
    fn numeric_tag_values_must_be_finite_numbers() {
        let derived = DerivedTag {
            tag: "Area1.LIC101.PV".to_string(),
            roles: vec!["process_variable".to_string()],
        };
        for raw in ["bad-value", "NaN", "inf"] {
            let read = assess_tag_read(
                &derived,
                Some(&TagValue {
                    tag: derived.tag.clone(),
                    value: raw.to_string(),
                    quality: Quality::Good,
                    timestamp: None,
                }),
                false,
                true,
            );
            assert_eq!(read.status, PreflightCheckStatus::Fail);
            assert!(read.detail.contains("not a number") || read.detail.contains("not a finite"));
        }
    }

    #[test]
    fn mode_checks_report_missing_values_and_attribute_program_mismatches() {
        let yokogawa = built_in_templates()
            .into_iter()
            .find(|template| template.name == "Yokogawa CentumVP")
            .unwrap();
        let honeywell = built_in_templates()
            .into_iter()
            .find(|template| template.name == "Honeywell Experion")
            .unwrap();
        let mode_tags = LoopTags::derive_from_pv_tag("Area1.LIC101.PV", &yokogawa);
        let mode_tag = mode_tags.controller_mode.as_deref().unwrap();
        assert_eq!(
            mode_status(&mode_tags, &yokogawa, &HashMap::new(), &[]).0,
            PreflightCheckStatus::Fail
        );
        let mode_read = PreflightTagRead {
            tag: mode_tag.to_string(),
            roles: vec!["controller_mode".to_string()],
            value: None,
            quality: None,
            status: PreflightCheckStatus::Pass,
            detail: String::new(),
        };
        assert_eq!(
            mode_status(
                &mode_tags,
                &yokogawa,
                &HashMap::new(),
                std::slice::from_ref(&mode_read)
            )
            .0,
            PreflightCheckStatus::Fail
        );
        let mut failed_mode_read = mode_read.clone();
        failed_mode_read.status = PreflightCheckStatus::Fail;
        assert_eq!(
            mode_status(
                &mode_tags,
                &yokogawa,
                &HashMap::new(),
                std::slice::from_ref(&failed_mode_read)
            )
            .0,
            PreflightCheckStatus::Fail
        );

        let mut invalid_attribute_template = honeywell.clone();
        invalid_attribute_template.mode_attribute_program_value = None;
        let attribute_tags = LoopTags::derive_from_pv_tag("Area1.LIC101.PV", &honeywell);
        assert_eq!(
            mode_attribute_status(&attribute_tags, &honeywell, &HashMap::new(), &[]).0,
            PreflightCheckStatus::Fail
        );
        assert_eq!(
            mode_attribute_status(
                &attribute_tags,
                &invalid_attribute_template,
                &HashMap::new(),
                &[]
            )
            .0,
            PreflightCheckStatus::Fail
        );
        let attribute_tag = attribute_tags.mode_attribute.as_deref().unwrap();
        let attribute_read = PreflightTagRead {
            tag: attribute_tag.to_string(),
            roles: vec!["mode_attribute".to_string()],
            value: None,
            quality: None,
            status: PreflightCheckStatus::Pass,
            detail: String::new(),
        };
        let mut failed_attribute_read = attribute_read.clone();
        failed_attribute_read.status = PreflightCheckStatus::Fail;
        assert_eq!(
            mode_attribute_status(
                &attribute_tags,
                &honeywell,
                &HashMap::new(),
                std::slice::from_ref(&failed_attribute_read)
            )
            .0,
            PreflightCheckStatus::Fail
        );
        assert_eq!(
            mode_attribute_status(
                &attribute_tags,
                &honeywell,
                &HashMap::new(),
                std::slice::from_ref(&attribute_read)
            )
            .0,
            PreflightCheckStatus::Fail
        );
        let uncertain_attribute = TagValue {
            tag: attribute_tag.to_string(),
            value: honeywell.mode_attribute_program_value.clone().unwrap(),
            quality: Quality::Uncertain,
            timestamp: None,
        };
        let mut values = HashMap::new();
        values.insert(attribute_tag.to_string(), uncertain_attribute);
        let mut warning_read = attribute_read;
        warning_read.status = PreflightCheckStatus::Warn;
        assert_eq!(
            mode_attribute_status(
                &attribute_tags,
                &honeywell,
                &values,
                std::slice::from_ref(&warning_read)
            )
            .0,
            PreflightCheckStatus::Warn
        );
    }

    #[test]
    fn writeback_readiness_reflects_missing_tags_quality_and_mode_attribute() {
        let request = test_request(DriverKind::Opcda, None).into_request();
        let template = built_in_templates()
            .into_iter()
            .find(|template| template.name == "Yokogawa CentumVP")
            .unwrap();
        let mut tags = build_loop_tags(&request, &template).unwrap();
        let reads: Vec<PreflightTagRead> = [
            tags.proportional_constant.as_ref(),
            tags.integral_constant.as_ref(),
            tags.derivative_constant.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(|tag| PreflightTagRead {
            tag: tag.clone(),
            roles: Vec::new(),
            value: Some("1".to_string()),
            quality: Some("Good".to_string()),
            status: PreflightCheckStatus::Pass,
            detail: String::new(),
        })
        .collect();

        assert_eq!(
            write_back_status(&request, &tags, &reads, PreflightCheckStatus::Pass),
            PreflightCheckStatus::Pass
        );
        assert!(
            write_back_detail(&request, &tags, &reads, PreflightCheckStatus::Pass)
                .contains("does not probe controller write permissions")
        );
        assert_eq!(
            write_back_status(&request, &tags, &reads, PreflightCheckStatus::Fail),
            PreflightCheckStatus::Fail
        );
        assert!(
            write_back_detail(&request, &tags, &reads, PreflightCheckStatus::Fail)
                .contains("readiness could not be confirmed")
        );
        let mut warning_reads = reads.clone();
        warning_reads[0].status = PreflightCheckStatus::Warn;
        assert!(
            write_back_detail(&request, &tags, &warning_reads, PreflightCheckStatus::Pass)
                .contains("write-back readiness is incomplete")
        );
        assert_eq!(
            write_back_status(&request, &tags, &reads, PreflightCheckStatus::Warn),
            PreflightCheckStatus::Warn
        );
        assert!(
            write_back_detail(&request, &tags, &reads, PreflightCheckStatus::Warn)
                .contains("differs from the template's program value")
        );

        tags.proportional_constant = None;
        assert_eq!(
            write_back_status(&request, &tags, &reads, PreflightCheckStatus::Pass),
            PreflightCheckStatus::Warn
        );
        assert!(
            write_back_detail(&request, &tags, &reads, PreflightCheckStatus::Pass)
                .contains("not configured")
        );
        let mut requested = request.clone();
        requested.write_pid = Some(ResponseLevel::Moderate);
        assert_eq!(
            write_back_status(&requested, &tags, &reads, PreflightCheckStatus::Pass),
            PreflightCheckStatus::Fail
        );
        assert!(
            write_back_detail(&requested, &tags, &reads, PreflightCheckStatus::Pass)
                .contains("cannot be prepared")
        );

        let mut simulator = test_request(DriverKind::Simulator, None).into_request();
        simulator.write_pid = Some(ResponseLevel::Moderate);
        let simulator_tags = build_loop_tags(&simulator, &template).unwrap();
        assert_eq!(
            write_back_status(&simulator, &simulator_tags, &[], PreflightCheckStatus::Pass),
            PreflightCheckStatus::Fail
        );
        assert!(
            write_back_detail(&simulator, &simulator_tags, &[], PreflightCheckStatus::Pass)
                .contains("not available")
        );
    }

    #[test]
    fn writeback_readiness_propagates_failed_and_warning_pid_reads() {
        let request = test_request(DriverKind::Opcda, Some(ResponseLevel::Moderate)).into_request();
        let template = built_in_templates()
            .into_iter()
            .find(|template| template.name == "Yokogawa CentumVP")
            .unwrap();
        let tags = build_loop_tags(&request, &template).unwrap();
        let mut reads: Vec<PreflightTagRead> = [
            tags.proportional_constant.as_ref(),
            tags.integral_constant.as_ref(),
            tags.derivative_constant.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(|tag| PreflightTagRead {
            tag: tag.clone(),
            roles: Vec::new(),
            value: Some("1".to_string()),
            quality: Some("Good".to_string()),
            status: PreflightCheckStatus::Pass,
            detail: String::new(),
        })
        .collect();
        reads[0].status = PreflightCheckStatus::Warn;
        assert_eq!(
            write_back_status(&request, &tags, &reads, PreflightCheckStatus::Pass),
            PreflightCheckStatus::Warn
        );
        assert!(
            write_back_detail(&request, &tags, &reads, PreflightCheckStatus::Pass)
                .contains("warning-level reads")
        );
        reads[0].status = PreflightCheckStatus::Fail;
        assert_eq!(
            write_back_status(&request, &tags, &reads, PreflightCheckStatus::Pass),
            PreflightCheckStatus::Fail
        );
        assert_eq!(
            write_back_status(&request, &tags, &[], PreflightCheckStatus::Warn),
            PreflightCheckStatus::Fail
        );
        assert_eq!(
            write_back_status(&request, &tags, &reads, PreflightCheckStatus::Warn),
            PreflightCheckStatus::Fail
        );
        assert!(
            write_back_detail(&request, &tags, &reads, PreflightCheckStatus::Warn)
                .contains("mode attribute")
        );
    }

    #[tokio::test]
    async fn operation_timeout_helper_covers_errors_and_timeouts() {
        let error = within_timeout(1, "test read", async {
            Err::<(), _>(anyhow::anyhow!("read failed"))
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("read failed"));

        let timeout = within_timeout(0, "test read", std::future::pending::<anyhow::Result<()>>())
            .await
            .unwrap_err();
        assert!(timeout.to_string().contains("timed out after 0s"));
    }

    #[tokio::test]
    async fn template_resolution_honors_database_origins_and_catalog_fallbacks() {
        let builtin = built_in_templates()
            .into_iter()
            .find(|template| template.name == "Yokogawa CentumVP")
            .unwrap();
        let now = chrono::DateTime::from_timestamp(0, 0).unwrap();

        let builtin_pool = bhtune_db::connect_in_memory().await.unwrap();
        let mut stale_builtin = builtin.clone();
        stale_builtin.process_variable_suffix = "STALE".to_string();
        DcsTemplateRow::insert(&builtin_pool, &stale_builtin, TemplateOrigin::Builtin, now)
            .await
            .unwrap();
        let resolved = resolve_template(&builtin.name, Some(&builtin_pool), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.source, "built-in template");
        assert_eq!(resolved.template.process_variable_suffix, "PV");

        let user_pool = bhtune_db::connect_in_memory().await.unwrap();
        let mut user_owned = builtin.clone();
        user_owned.process_variable_suffix = "USERPV".to_string();
        DcsTemplateRow::insert(&user_pool, &user_owned, TemplateOrigin::User, now)
            .await
            .unwrap();
        let resolved = resolve_template(&builtin.name, Some(&user_pool), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.source, "database user template");
        assert_eq!(resolved.template.process_variable_suffix, "USERPV");

        let mut catalog_row = builtin.clone();
        catalog_row.name = "Catalog Template".to_string();
        catalog_row.process_variable_suffix = "OLDPV".to_string();
        let catalog_pool = bhtune_db::connect_in_memory().await.unwrap();
        DcsTemplateRow::insert(&catalog_pool, &catalog_row, TemplateOrigin::Catalog, now)
            .await
            .unwrap();
        let mut current_catalog = catalog_row.clone();
        current_catalog.process_variable_suffix = "NEWPV".to_string();
        let resolved = resolve_template(
            &catalog_row.name,
            Some(&catalog_pool),
            Some(std::slice::from_ref(&current_catalog)),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(resolved.source, "user catalog template");
        assert_eq!(resolved.template.process_variable_suffix, "NEWPV");

        let resolved = resolve_template(&catalog_row.name, Some(&catalog_pool), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.template.process_variable_suffix, "OLDPV");

        let resolved = resolve_template(
            &catalog_row.name,
            None,
            Some(std::slice::from_ref(&current_catalog)),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(resolved.source, "user catalog template");
        assert_eq!(resolved.template.process_variable_suffix, "NEWPV");

        assert!(
            resolve_template("Unknown", None, None)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn invalid_tuning_configuration_is_reported_before_driver_creation() {
        let mut config = BhtuneConfig::default();
        config.tuning.poll_interval_ms = Some(0);
        let templates = built_in_templates();
        let report = preflight(
            test_request(DriverKind::Simulator, None),
            &config,
            None,
            Some(&templates),
        )
        .await
        .unwrap();

        assert_eq!(report.status(), PreflightCheckStatus::Fail);
        assert!(report.tag_reads.is_empty());
        assert!(report.checks.iter().any(|check| {
            check.name == "Configuration and tuning" && check.status == PreflightCheckStatus::Fail
        }));
    }
}
