#![allow(rustdoc::broken_intra_doc_links)]

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::model::{BhtuneConfig, default_allow_uncertain_quality, parse_config_contents};
use super::paths::config_path_from;
use super::tuning::{
    TuningConfig, TuningConfigSources, resolve_and_validate_tuning_config, tuning_config_sources,
};

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
                            .and_then(toml_edit::Value::as_bool)
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

#[cfg(test)]
mod tests {
    use super::super::log::LogConfig;
    use super::super::tuning::TuningConfigSource;
    use super::*;
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
}
