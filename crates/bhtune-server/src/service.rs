//! Platform service registration and lifecycle management (`server-windows-service`).
//!
//! Only the imperative Windows Service Control Manager (SCM) glue is
//! `#[cfg(target_os = "windows")]` -- it is invisible to the Linux/macOS coverage runs, so it
//! is kept as thin as possible. Everything that can be plain, platform-neutral logic (the
//! service's identity/definition, how CLI flags become launch arguments, the reporting order
//! of the SCM lifecycle, and how a "not launched by the SCM" failure is recognized) lives at
//! the top of this file, is exercised by the tests below on every platform (including CI's
//! `windows-latest` job, which compiles and runs the `#[cfg(windows)]` section too -- see
//! `.github/workflows/checks.yml`), and is only *mapped onto* the real `windows_service`
//! types inside the Windows-only section. Mirrors `opcda-bridge-gateway`'s own `service.rs`
//! (same crate, same design), generalized for a binary that -- unlike that Windows-only
//! gateway -- genuinely runs cross-platform.
//!
//! Linux and macOS have no equivalent self-registration API: the idiomatic path there is a
//! static unit/plist file an administrator (or a future `.deb`/`.rpm`/Homebrew package)
//! installs with the OS's own tooling, not something this binary does to itself at runtime --
//! see `packaging/systemd/bhtune-server.service` and
//! `packaging/launchd/com.bytehound-labs.bhtune-server.plist`. So on those platforms, this
//! module's public functions are still real (not `#[cfg(windows)]`-gated away, so
//! `bhtune-server install` on Linux fails with a helpful message rather than clap rejecting
//! an unrecognized subcommand outright), but they only explain that and point at the
//! relevant packaging file instead of touching anything.

use crate::cli::Cli;
use std::path::PathBuf;

/// Service name registered with the SCM (used for `sc query`, event log sourcing, etc. --
/// must contain no spaces).
pub const SERVICE_NAME: &str = "BhtuneServer";
/// Human-readable name shown in `services.msc`.
pub const SERVICE_DISPLAY_NAME: &str = "BHTune Server";
/// Shown as the service's description in `services.msc`.
pub const SERVICE_DESCRIPTION: &str = "Serves BHTune's HTTP API and embedded web GUI for MRFT PID auto-tuning. \
     https://github.com/bytehound-labs/bhtune";

/// Plain, platform-neutral description of how the server should be registered with the SCM.
/// Built and tested independent of the Windows-only `windows_service::service::ServiceInfo`
/// it is later mapped onto one field at a time, so this construction logic runs -- and is
/// covered -- on every platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceDefinition {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub executable_path: PathBuf,
    pub launch_arguments: Vec<String>,
}

/// Re-serialize whichever CLI flags were given to `install` into the argument list the SCM
/// should launch the executable with. The SCM always starts a service's executable bare (no
/// interactive shell, no inherited environment beyond the system default -- notably including
/// a *different* `%APPDATA%` than whichever user ran `install` interactively, since services
/// typically run under their own account), so an explicit `--config` an operator wants
/// applied every time the service starts must be baked into the registration itself rather
/// than relying on how the executable happened to be invoked once at install time -- see
/// `crate::cli`'s module doc comment.
pub fn service_launch_arguments(cli: &Cli) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(config) = &cli.config {
        args.push("--config".to_string());
        args.push(config.display().to_string());
    }
    args
}

/// Build the platform-neutral service definition used by `install`, pairing the current
/// executable's path with whichever CLI flags should carry over into the service's own
/// launch.
pub fn build_service_definition(executable_path: PathBuf, cli: &Cli) -> ServiceDefinition {
    ServiceDefinition {
        name: SERVICE_NAME.to_string(),
        display_name: SERVICE_DISPLAY_NAME.to_string(),
        description: SERVICE_DESCRIPTION.to_string(),
        executable_path,
        launch_arguments: service_launch_arguments(cli),
    }
}

/// The SCM status lifecycle `bhtune-server` reports while running as a Windows service, kept
/// as a plain enum (rather than directly using `windows_service::service::ServiceState`,
/// which only exists on Windows) purely so the expected reporting order is itself
/// unit-testable on every platform. The Windows-only reporting code maps each variant onto
/// the real SCM API one-to-one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceLifecycle {
    /// Reported immediately after registering the control handler, while config/logging/the
    /// database are still being resolved and the listener has not bound yet.
    StartPending,
    /// Reported once [`crate::run::build_server`] has returned -- the listener is bound and
    /// ready to serve.
    Running,
    /// Reported the instant a Stop/Shutdown control event arrives, before in-flight requests
    /// have finished draining.
    StopPending,
    /// Reported after the server has fully drained and [`crate::run::serve`] has returned.
    Stopped,
}

impl ServiceLifecycle {
    /// The state that follows this one in the fixed reporting sequence, or `None` after
    /// `Stopped` (the sequence's end). Encodes -- and lets tests lock in -- the intended
    /// order without needing the Windows-only types the real reporting code sends to the SCM.
    pub fn next(self) -> Option<Self> {
        match self {
            ServiceLifecycle::StartPending => Some(ServiceLifecycle::Running),
            ServiceLifecycle::Running => Some(ServiceLifecycle::StopPending),
            ServiceLifecycle::StopPending => Some(ServiceLifecycle::Stopped),
            ServiceLifecycle::Stopped => None,
        }
    }
}

/// Windows' `ERROR_FAILED_SERVICE_CONTROLLER_CONNECT`: the Win32 error code
/// `StartServiceCtrlDispatcherW` returns when the calling process was launched interactively
/// rather than by the Service Control Manager.
const ERROR_FAILED_SERVICE_CONTROLLER_CONNECT: i32 = 1063;

/// True when `code` is the raw OS error that means "this process wasn't started by the SCM"
/// -- i.e. `main` should fall back to running the server directly in the foreground rather
/// than treating this as a real failure. Kept as a plain function over the numeric code
/// (rather than matching directly on `windows_service::Error`, which only exists on Windows)
/// so this small but important piece of "which failure means fall back to console mode"
/// logic is still covered by the cross-platform test run.
pub fn is_scm_launch_error_code(code: Option<i32>) -> bool {
    code == Some(ERROR_FAILED_SERVICE_CONTROLLER_CONNECT)
}

/// Explains why a `service`-management subcommand can't do anything on this platform, and
/// where the real equivalent lives instead. Shared by every non-Windows stub below so the
/// message only needs to be written once.
#[cfg(not(target_os = "windows"))]
fn platform_service_error(action: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "`bhtune-server {action}` manages a Windows service and only works on Windows.\n\
         On Linux, install the provided systemd unit instead:\n  \
         packaging/systemd/bhtune-server.service\n\
         On macOS, install the provided launchd daemon instead:\n  \
         packaging/launchd/com.bytehound-labs.bhtune-server.plist\n\
         See docs/getting-started/installation.md#run-as-a-background-service for the exact \
         steps."
    )
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::*;
    use crate::cli::Cli;
    use crate::run;
    use std::ffi::OsString;
    use std::time::Duration;
    use windows_service::service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
        ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::ServiceStatusHandle;
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_dispatcher;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

    windows_service::define_windows_service!(ffi_service_main, service_main);

    /// Maps a [`ServiceDefinition`] onto the real, Windows-only `ServiceInfo` the SCM API
    /// needs. Deliberately trivial -- all the actual decision-making already happened in
    /// [`build_service_definition`].
    fn to_service_info(definition: &ServiceDefinition) -> ServiceInfo {
        ServiceInfo {
            name: OsString::from(&definition.name),
            display_name: OsString::from(&definition.display_name),
            service_type: SERVICE_TYPE,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: definition.executable_path.clone(),
            launch_arguments: definition
                .launch_arguments
                .iter()
                .map(OsString::from)
                .collect(),
            dependencies: vec![],
            account_name: None, // Run as LocalSystem.
            account_password: None,
        }
    }

    /// Registers `bhtune-server` with the SCM (does not start it).
    pub fn install(cli: &Cli) -> anyhow::Result<()> {
        let exe = std::env::current_exe()?;
        let definition = build_service_definition(exe, cli);
        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )?;
        let service =
            manager.create_service(&to_service_info(&definition), ServiceAccess::CHANGE_CONFIG)?;
        service.set_description(&definition.description)?;
        println!(
            "Installed '{}' ({}). Start it with: bhtune-server.exe start",
            definition.display_name, definition.name
        );
        Ok(())
    }

    /// Stops (if running) and removes the registered service.
    pub fn uninstall() -> anyhow::Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(
            SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
        )?;
        if service.query_status()?.current_state != ServiceState::Stopped {
            service.stop()?;
        }
        service.delete()?;
        println!("Uninstalled '{SERVICE_DISPLAY_NAME}'.");
        Ok(())
    }

    /// Starts the registered service.
    pub fn start() -> anyhow::Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(SERVICE_NAME, ServiceAccess::START)?;
        service.start::<&std::ffi::OsStr>(&[])?;
        println!("Started '{SERVICE_DISPLAY_NAME}'.");
        Ok(())
    }

    /// Requests the running service to stop.
    pub fn stop() -> anyhow::Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(SERVICE_NAME, ServiceAccess::STOP)?;
        service.stop()?;
        println!("Stop requested for '{SERVICE_DISPLAY_NAME}'.");
        Ok(())
    }

    /// Prints the registered service's current SCM state.
    pub fn status() -> anyhow::Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)?;
        let status = service.query_status()?;
        println!("{SERVICE_DISPLAY_NAME}: {:?}", status.current_state);
        Ok(())
    }

    /// True when `err` is the specific failure `service_dispatcher::start` returns when this
    /// process was launched interactively rather than by the SCM -- the signal that `main`
    /// should fall back to console mode.
    pub fn is_run_outside_scm(err: &windows_service::Error) -> bool {
        matches!(
            err,
            windows_service::Error::Winapi(io_err) if is_scm_launch_error_code(io_err.raw_os_error())
        )
    }

    /// Registers the generated service entry point with the SCM and blocks until the service
    /// stops. Returns immediately with an error -- no threads spawned, nothing torn down -- if
    /// this process was not actually launched by the SCM; see [`is_run_outside_scm`].
    pub fn run_as_service() -> windows_service::Result<()> {
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
    }

    /// Reports one step of the server's SCM lifecycle. `controls_accepted` is only
    /// meaningful while `Running` -- a service in a pending state cannot yet (or any longer)
    /// accept control events. `wait_hint` gives the SCM how long to wait before considering
    /// the service hung; the generous `StopPending` hint gives in-flight HTTP requests and an
    /// active tune's cancel/restore time to finish, matching [`run::serve`]'s own
    /// graceful-shutdown behavior (see `SHUTDOWN_RUN_CANCEL_TIMEOUT`).
    fn report_status(
        handle: &ServiceStatusHandle,
        state: ServiceLifecycle,
    ) -> windows_service::Result<()> {
        let (current_state, controls_accepted, wait_hint) = match state {
            ServiceLifecycle::StartPending => (
                ServiceState::StartPending,
                ServiceControlAccept::empty(),
                Duration::from_secs(10),
            ),
            ServiceLifecycle::Running => (
                ServiceState::Running,
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
                Duration::default(),
            ),
            ServiceLifecycle::StopPending => (
                ServiceState::StopPending,
                ServiceControlAccept::empty(),
                Duration::from_secs(40),
            ),
            ServiceLifecycle::Stopped => (
                ServiceState::Stopped,
                ServiceControlAccept::empty(),
                Duration::default(),
            ),
        };
        handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state,
            controls_accepted,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint,
            process_id: None,
        })
    }

    /// The service entry point invoked by the SCM on a background thread. `_arguments` is the
    /// SCM's secondary start-parameter channel (e.g. an operator running `sc start name
    /// extra`) -- distinct from the process's real argv, which is what `Cli::parse()` inside
    /// `run_service` sees, identically to console-mode startup, since it's the same launch
    /// command `install` registered.
    fn service_main(_arguments: Vec<OsString>) {
        if let Err(e) = run_service() {
            // No console and, if this failed early, possibly no SCM status handle either --
            // file logging (once `run::build_server` initializes it) is the real record of
            // this; stderr is a last-resort breadcrumb.
            eprintln!("bhtune-server service run failed: {e:?}");
        }
    }

    fn run_service() -> anyhow::Result<()> {
        use clap::Parser;

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let shutdown_tx = std::sync::Mutex::new(Some(shutdown_tx));

        let event_handler = move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                // All services must accept Interrogate even as a no-op.
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    if let Some(tx) = shutdown_tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
                        let _ = tx.send(());
                    }
                    ServiceControlHandlerResult::NoError
                }
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };

        let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;
        report_status(&status_handle, ServiceLifecycle::StartPending)?;

        // Real launch arguments (baked in by `install`), not the SCM's secondary
        // `_arguments` parameter above.
        let cli = Cli::parse();

        let rt = tokio::runtime::Runtime::new()?;
        // `build_server` runs the full startup sequence (config/log/db, migrations, bind) --
        // only once it returns is the listener actually bound and ready, which is the
        // moment `Running` becomes true, not a moment earlier (contrast
        // `opcda-bridge-gateway`'s own `service_main`, which reports `Running` right after
        // parsing CLI args since its own bootstrap has no comparable async setup cost).
        let server = rt.block_on(run::build_server(cli.config.as_deref()))?;
        report_status(&status_handle, ServiceLifecycle::Running)?;

        // Reports `StopPending` the instant the stop signal arrives, before `run::serve`'s
        // shutdown future actually resolves and the in-flight-request drain begins -- this
        // is exactly the moment the SCM needs to stop expecting an immediate `Stopped`.
        // `ServiceStatusHandle` is `Copy` (and documented safe to use from any thread), so
        // this is a plain copy, not a deep clone.
        let stop_status_handle = status_handle;
        let shutdown = async move {
            let _ = shutdown_rx.await;
            let _ = report_status(&stop_status_handle, ServiceLifecycle::StopPending);
        };

        let result = rt.block_on(run::serve(server, shutdown));

        report_status(&status_handle, ServiceLifecycle::Stopped)?;
        result
    }
}

#[cfg(target_os = "windows")]
pub use windows_impl::{
    install, is_run_outside_scm, run_as_service, start, status, stop, uninstall,
};

#[cfg(not(target_os = "windows"))]
pub fn install(_cli: &Cli) -> anyhow::Result<()> {
    Err(platform_service_error("install"))
}

#[cfg(not(target_os = "windows"))]
pub fn uninstall() -> anyhow::Result<()> {
    Err(platform_service_error("uninstall"))
}

#[cfg(not(target_os = "windows"))]
pub fn start() -> anyhow::Result<()> {
    Err(platform_service_error("start"))
}

#[cfg(not(target_os = "windows"))]
pub fn stop() -> anyhow::Result<()> {
    Err(platform_service_error("stop"))
}

#[cfg(not(target_os = "windows"))]
pub fn status() -> anyhow::Result<()> {
    Err(platform_service_error("status"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn cli_with(config: Option<&str>) -> Cli {
        Cli {
            command: None,
            config: config.map(PathBuf::from),
        }
    }

    #[test]
    fn service_launch_arguments_empty_when_no_config_flag_set() {
        let cli = cli_with(None);
        assert_eq!(service_launch_arguments(&cli), Vec::<String>::new());
    }

    #[test]
    fn service_launch_arguments_includes_the_config_flag_when_set() {
        let cli = cli_with(Some("/etc/bhtune/bhtune.toml"));
        assert_eq!(
            service_launch_arguments(&cli),
            vec![
                "--config".to_string(),
                "/etc/bhtune/bhtune.toml".to_string(),
            ]
        );
    }

    #[test]
    fn build_service_definition_carries_identity_and_arguments() {
        let cli = cli_with(Some("C:\\ProgramData\\bhtune\\bhtune.toml"));
        let definition = build_service_definition(PathBuf::from("C:\\bhtune-server.exe"), &cli);
        assert_eq!(definition.name, SERVICE_NAME);
        assert_eq!(definition.display_name, SERVICE_DISPLAY_NAME);
        assert_eq!(definition.description, SERVICE_DESCRIPTION);
        assert_eq!(
            definition.executable_path,
            PathBuf::from("C:\\bhtune-server.exe")
        );
        assert_eq!(
            definition.launch_arguments,
            vec![
                "--config".to_string(),
                "C:\\ProgramData\\bhtune\\bhtune.toml".to_string(),
            ]
        );
    }

    #[test]
    fn build_service_definition_with_no_flags_has_no_launch_arguments() {
        let cli = cli_with(None);
        let definition = build_service_definition(PathBuf::from("/usr/bin/bhtune-server"), &cli);
        assert_eq!(definition.launch_arguments, Vec::<String>::new());
    }

    #[test]
    fn service_lifecycle_sequence_order() {
        assert_eq!(
            ServiceLifecycle::StartPending.next(),
            Some(ServiceLifecycle::Running)
        );
        assert_eq!(
            ServiceLifecycle::Running.next(),
            Some(ServiceLifecycle::StopPending)
        );
        assert_eq!(
            ServiceLifecycle::StopPending.next(),
            Some(ServiceLifecycle::Stopped)
        );
    }

    #[test]
    fn service_lifecycle_stopped_is_terminal() {
        assert_eq!(ServiceLifecycle::Stopped.next(), None);
    }

    #[test]
    fn is_scm_launch_error_code_matches_expected_code() {
        assert!(is_scm_launch_error_code(Some(1063)));
    }

    #[test]
    fn is_scm_launch_error_code_rejects_other_codes() {
        assert!(!is_scm_launch_error_code(Some(5)));
        assert!(!is_scm_launch_error_code(None));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_stubs_name_the_action_and_point_at_packaging() {
        for (action, result) in [
            ("install", install(&cli_with(None))),
            ("uninstall", uninstall()),
            ("start", start()),
            ("stop", stop()),
            ("status", status()),
        ] {
            let message = result.unwrap_err().to_string();
            assert!(
                message.contains(&format!("bhtune-server {action}")),
                "message for {action} should name the action verbatim: {message}"
            );
            assert!(message.contains("packaging/systemd/bhtune-server.service"));
            assert!(message.contains("packaging/launchd/"));
        }
    }
}
