//! `bhtune-server`'s CLI surface (`server-windows-service`): a `--config` flag plus five
//! subcommands (`install`/`uninstall`/`start`/`stop`/`status`) that manage this binary's
//! registration as a platform service.
//!
//! Kept deliberately tiny -- unlike `bhtune-cli`, this binary is still meant to be run mostly
//! unconfigured (env vars / `bhtune.toml` cover everything else, see `crate::run`). `--config`
//! is the one flag worth adding now: it lets `install` bake an explicit, stable config path
//! into the service's registered launch command (see `crate::service::service_launch_arguments`),
//! which matters because a Windows service normally runs under a different account than
//! whoever ran `install` interactively, so it would otherwise resolve `%APPDATA%` to a
//! different (system) profile and appear to have "lost" any config/database an operator set
//! up while testing from their own terminal session -- see
//! `docs/getting-started/installation.md`'s "Run as a background service" section.
//!
//! `ServiceCommand` is parsed identically on every platform (so `--help` output and argument
//! validation are covered by CI's Windows *and* Linux jobs alike), even though only Windows
//! can actually act on it -- see `crate::service`'s module doc comment for how the two other
//! platforms respond instead.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Manage `bhtune-server`'s registration as a platform service.
#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceCommand {
    /// Register bhtune-server as a Windows service (does not start it).
    Install,
    /// Stop (if running) and remove the registered service.
    Uninstall,
    /// Start the registered service.
    Start,
    /// Request the running service to stop.
    Stop,
    /// Print the registered service's current state.
    Status,
}

/// The `bhtune-server` binary's command line.
#[derive(Parser, Debug)]
#[command(
    name = "bhtune-server",
    version,
    about = "BHTune's HTTP API and embedded web GUI"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<ServiceCommand>,

    /// Path to a TOML config file (default: platform-specific, see `bhtune_cli::config`).
    /// Baked into the registered launch command by `install` -- see this module's doc
    /// comment.
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_parses_to_no_subcommand_and_no_config() {
        let cli = Cli::parse_from(["bhtune-server"]);
        assert_eq!(cli.command, None);
        assert_eq!(cli.config, None);
    }

    #[test]
    fn config_flag_parses_without_a_subcommand() {
        let cli = Cli::parse_from(["bhtune-server", "--config", "/etc/bhtune/bhtune.toml"]);
        assert_eq!(cli.command, None);
        assert_eq!(cli.config, Some(PathBuf::from("/etc/bhtune/bhtune.toml")));
    }

    #[test]
    fn every_service_subcommand_parses() {
        for (arg, expected) in [
            ("install", ServiceCommand::Install),
            ("uninstall", ServiceCommand::Uninstall),
            ("start", ServiceCommand::Start),
            ("stop", ServiceCommand::Stop),
            ("status", ServiceCommand::Status),
        ] {
            let cli = Cli::parse_from(["bhtune-server", arg]);
            assert_eq!(cli.command, Some(expected));
        }
    }

    #[test]
    fn install_carries_a_config_flag_given_after_the_subcommand() {
        // `--config` is `global = true`, so clap accepts it either before or after the
        // subcommand -- `install` (the only caller that reads it, via
        // `crate::service::service_launch_arguments`) needs the latter to work too.
        let cli = Cli::parse_from([
            "bhtune-server",
            "install",
            "--config",
            "C:\\ProgramData\\bhtune\\bhtune.toml",
        ]);
        assert_eq!(cli.command, Some(ServiceCommand::Install));
        assert_eq!(
            cli.config,
            Some(PathBuf::from("C:\\ProgramData\\bhtune\\bhtune.toml"))
        );
    }
}
