//! `bhtune-server` binary: a thin bootstrap shell, mirroring `bhtune-cli`'s own `main.rs`/
//! `lib.rs::run` split -- see `bhtune_server`'s crate doc comment for why the actual routes
//! live in the lib target instead.
//!
//! Platform-split, following the exact same shape `opcda-bridge-gateway`'s `main.rs`
//! already proves out (`server-windows-service`): on Windows, `main` must be a plain,
//! synchronous `fn` -- `windows_service::service_dispatcher::start` (called from
//! `service::run_as_service`) cannot be invoked from inside an already-running Tokio
//! runtime, so no `#[tokio::main]` here; the interactive fallback and the real Windows
//! service path (`crate::service::windows_impl::run_service`) each construct their own
//! `tokio::runtime::Runtime` instead. Every other platform has no such constraint and no
//! self-registration API to dispatch to (see `crate::service`'s module doc comment), so its
//! `main` is a plain `#[tokio::main] async fn` that always runs the server directly --
//! unlike the Windows-only `opcda-bridge-gateway`, whose non-Windows `main` only errors,
//! bhtune-server is genuinely cross-platform and must actually serve requests there.
//!
//! `ServiceCommand` dispatch (`install`/`uninstall`/`start`/`stop`/`status`) is identical on
//! every platform -- `dispatch_service_command` is plain, `#[cfg]`-free code, since
//! `crate::service`'s exported functions have the same signatures whichever platform's
//! implementation (the real Windows one, or the explanatory non-Windows stub) got compiled
//! in.

#[cfg(target_os = "windows")]
use std::path::Path;

use bhtune_server::cli::{Cli, ServiceCommand};
use bhtune_server::{run, service};
use clap::Parser;

#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(command) = cli.command {
        return dispatch_service_command(command, &cli);
    }

    // No subcommand given: this is exactly how the SCM launches a running service (the
    // launch arguments `install` bakes in never include a subcommand -- see
    // `crate::service::service_launch_arguments`), so try that path first. Falls back to
    // ordinary interactive/foreground mode when `service_dispatcher::start` reports this
    // process wasn't actually started by the SCM (see `service::is_run_outside_scm`) -- the
    // common case when running the built exe directly from a terminal.
    match service::run_as_service() {
        Ok(()) => Ok(()),
        Err(e) if service::is_run_outside_scm(&e) => run_interactive(cli.config.as_deref()),
        Err(e) => Err(e.into()),
    }
}

/// Runs the server in the foreground, blocking until a shutdown signal arrives. Only needed
/// on Windows, where `main` is synchronous (see this module's doc comment) and must
/// construct its own runtime for this fallback path -- every other platform's `main` is
/// already async via `#[tokio::main]` and calls `run::build_server`/`run::serve` directly.
#[cfg(target_os = "windows")]
fn run_interactive(config_path: Option<&Path>) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let server = run::build_server(config_path).await?;
        run::serve(server, run::shutdown_signal()).await
    })
}

#[cfg(not(target_os = "windows"))]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(command) = cli.command {
        return dispatch_service_command(command, &cli);
    }

    let server = run::build_server(cli.config.as_deref()).await?;
    run::serve(server, run::shutdown_signal()).await
}

/// Runs one of the `install`/`uninstall`/`start`/`stop`/`status` subcommands. Plain,
/// `#[cfg]`-free code shared by every platform's `main` above -- `crate::service`'s exported
/// functions have the same signatures on Windows (the real SCM glue) and elsewhere (stubs
/// explaining that platform's actual equivalent, see `crate::service`'s module doc comment),
/// so this dispatch itself never needs to know which one it's calling.
fn dispatch_service_command(command: ServiceCommand, cli: &Cli) -> anyhow::Result<()> {
    match command {
        ServiceCommand::Install => service::install(cli),
        ServiceCommand::Uninstall => service::uninstall(),
        ServiceCommand::Start => service::start(),
        ServiceCommand::Stop => service::stop(),
        ServiceCommand::Status => service::status(),
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;

    #[test]
    fn dispatches_each_service_command_to_the_non_windows_stub() {
        let cli = Cli {
            command: None,
            config: None,
        };
        for (command, action) in [
            (ServiceCommand::Install, "install"),
            (ServiceCommand::Uninstall, "uninstall"),
            (ServiceCommand::Start, "start"),
            (ServiceCommand::Stop, "stop"),
            (ServiceCommand::Status, "status"),
        ] {
            let error = dispatch_service_command(command, &cli).unwrap_err();
            assert!(
                error.to_string().contains(action),
                "stub error should identify {action}: {error}"
            );
        }
    }
}
