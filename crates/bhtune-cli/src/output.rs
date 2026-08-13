//! Output format selection for the handful of commands that support `--output json`
//! (`history list`/`history show` and `tune`/`simulate`'s final summary line — see
//! AGENTS.md's `cli-automation` section for why exactly these three).
//!
//! Mirrors `opcda-bridge-client`'s `output.rs` in spirit (a small `OutputFormat` enum plus a
//! `format_error` helper), but deliberately without its generic `render<T: Tabled +
//! Serialize>` function: bhtune-cli's commands print bespoke, multi-section reports (`history
//! show`'s run detail, `tune`'s calculated-PID listing), not flat single-row-type tables, so
//! there is no one shared row shape to hand to a generic renderer. Each command instead
//! builds its own JSON-serializable summary type and calls `serde_json::to_string_pretty`
//! directly — see `commands::history`/`commands::tune`.

use clap::ValueEnum;

/// How a command's result is printed. Only commands documented in this module's doc comment
/// honor this; every other command always prints its existing human-readable text.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable text (default).
    #[default]
    Table,
    /// Pretty-printed JSON. This is the external contract for scripted/scheduled consumers,
    /// so its shape must not change silently once shipped.
    Json,
}

/// Format an error for display, matching the requested output format.
///
/// The `Table` branch reproduces this crate's existing plain-text error format
/// (`"error: {err:#}"`, anyhow's flattened `Display` chain), so plain-text users see the same
/// text as before `--output` existed. The `Json` branch emits `{"error": "<message>"}` so
/// scripted consumers never have to parse free-text stderr.
pub fn format_error(err: &anyhow::Error, format: OutputFormat) -> String {
    match format {
        OutputFormat::Table => format!("error: {err:#}"),
        OutputFormat::Json => {
            let payload = serde_json::json!({ "error": err.to_string() });
            serde_json::to_string_pretty(&payload)
                .unwrap_or_else(|_| format!("{{\"error\": \"{err}\"}}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_format_defaults_to_table() {
        assert_eq!(OutputFormat::default(), OutputFormat::Table);
    }

    #[test]
    fn format_error_table_matches_existing_plain_text_format() {
        let err = anyhow::anyhow!("boom");
        assert_eq!(
            format_error(&err, OutputFormat::Table),
            format!("error: {err:#}")
        );
    }

    #[test]
    fn format_error_json_is_valid_json_with_message() {
        let err = anyhow::anyhow!("boom");
        let out = format_error(&err, OutputFormat::Json);
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["error"], "boom");
    }

    #[test]
    fn format_error_json_is_pretty_printed() {
        let err = anyhow::anyhow!("boom");
        let out = format_error(&err, OutputFormat::Json);
        assert!(out.contains('\n'), "expected multi-line pretty JSON");
    }

    #[test]
    fn format_error_preserves_anyhows_context_chain() {
        let err = anyhow::anyhow!("root cause").context("higher-level context");
        let table = format_error(&err, OutputFormat::Table);
        assert!(table.contains("root cause"));
        assert!(table.contains("higher-level context"));
    }
}
