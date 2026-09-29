#![allow(rustdoc::broken_intra_doc_links)]

use serde::{Deserialize, Serialize};

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
