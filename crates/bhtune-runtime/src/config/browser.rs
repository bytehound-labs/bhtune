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
mod tests;
