#![allow(rustdoc::broken_intra_doc_links)]

use std::path::PathBuf;

use super::model::BhtuneConfig;
use super::paths::templates_path_from;

/// Resolve and load the user-supplied DCS/PLC template catalog:
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
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
pid_rounding = {{ kind = "significant_digits", digits = 3 }}
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
}
