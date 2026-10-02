#![allow(rustdoc::broken_intra_doc_links)]

use std::path::{Path, PathBuf};

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

/// Derive bhtune's default *user template catalog* location the same way [`config_path_from`]
/// derives `bhtune.toml`'s -- deliberately the same directory, since both are per-user
/// settings a site admin edits by hand, not persistent application data (contrast
/// [`default_db_path_from`]/[`default_log_dir_from`], which live under the platform data
/// directory instead). See [`load_user_templates`] for how this default fits into the full
/// configuration precedence chain.
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
