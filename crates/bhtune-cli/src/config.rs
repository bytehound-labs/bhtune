//! `CLI flag > env var > TOML config file > built-in default` precedence for bhtune's global
//! settings (database location, opcda-bridge gateway address, default OPC server), mirroring
//! `opcda-bridge-client`'s `config.rs` (see AGENTS.md's `cli-config` notes) so both projects'
//! configuration surfaces stay recognizable to the same user.

use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
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

/// bhtune's configuration, loaded from an optional TOML file. Every field is optional; a
/// value missing from the file (or the file itself missing) falls back to the env var / CLI
/// flag / built-in default resolution in the `resolve_*` functions below.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct BhtuneConfig {
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
            db: None,
            bridge_host: None,
            server: None,
            templates: None,
            bind: None,
            retention_days: None,
            allow_uncertain_quality: default_allow_uncertain_quality(),
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
}

/// The two config-page-owned settings that can be patched in place while preserving every
/// unrelated key and comment in the source TOML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPolicyUpdate {
    pub allow_uncertain_quality: bool,
    pub retention_days: Option<u32>,
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
                let revision = revision_token_for_raw(Some(&raw));
                Ok(LoadedConfigStore {
                    path: Some(path),
                    missing_is_allowed: resolution.missing_is_allowed,
                    original_raw: Some(raw),
                    config,
                    revision,
                    toml_allow_uncertain_quality,
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

fn patch_config_policy(
    raw: Option<&str>,
    update: &ConfigPolicyUpdate,
) -> Result<(String, BhtuneConfig), String> {
    patch_config_contents(raw, |document| {
        document["allow_uncertain_quality"] = toml_edit::value(update.allow_uncertain_quality);
        match update.retention_days {
            Some(days) => {
                document["retention_days"] = toml_edit::value(i64::from(days));
            }
            None => {
                document.as_table_mut().remove("retention_days");
            }
        }
    })
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
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|e| ConfigStoreError::Write {
            path: path.to_path_buf(),
            action: "create config directory",
            source: e,
        })?;
    }
    Ok(())
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

    Ok(ConfigSaveResult {
        backup_path,
        state: LoadedConfigStore {
            path: Some(path),
            missing_is_allowed: state.missing_is_allowed,
            original_raw: Some(patched_raw),
            config,
            revision,
            toml_allow_uncertain_quality,
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
            level in prop::option::of("[A-Za-z0-9_.:-]{0,16}"),
            dir in prop::option::of("[A-Za-z0-9_./:-]{0,32}"),
            format in prop::option::of("[A-Za-z0-9_.:-]{0,16}"),
            rotation in prop::option::of("[A-Za-z0-9_.:-]{0,16}"),
        ) {
            let config = BhtuneConfig {
                db: db.map(PathBuf::from),
                bridge_host,
                server,
                templates: templates.map(PathBuf::from),
                bind,
                retention_days,
                allow_uncertain_quality,
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
            },
        )
        .unwrap();

        assert_eq!(result.backup_path, None);
        assert!(expected_path.exists());
        assert_eq!(result.state.path, Some(expected_path.clone()));
        assert_eq!(result.state.config.retention_days, Some(14));
        assert!(!result.state.config.allow_uncertain_quality);
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
        };
        let err = save_config_store(
            &state,
            &state.revision,
            &ConfigPolicyUpdate {
                allow_uncertain_quality: true,
                retention_days: None,
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
        };
        let err = save_config_store(
            &appeared,
            &appeared.revision,
            &ConfigPolicyUpdate {
                allow_uncertain_quality: true,
                retention_days: None,
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
        };
        let err = save_config_store(
            &unreadable,
            &unreadable.revision,
            &ConfigPolicyUpdate {
                allow_uncertain_quality: true,
                retention_days: None,
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
        };
        let err = save_config_store(
            &malformed,
            &malformed.revision,
            &ConfigPolicyUpdate {
                allow_uncertain_quality: true,
                retention_days: None,
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
}
