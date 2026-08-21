//! Spawns the real, compiled `bhtune-server` binary, connects to it over a genuine TCP
//! socket, then sends it a real `SIGTERM`/`SIGINT` and confirms it drains and exits cleanly.
//!
//! This is the one thing no in-process `tower::ServiceExt::oneshot` route test can exercise:
//! `main.rs`'s own bootstrap (config/log/db resolution, `TcpListener::bind`,
//! `axum::serve`) and `shutdown_signal()`'s real OS signal handling never run at all under
//! `oneshot`, which drives the router directly against an in-memory request/response pair.
//! Mirrors `bhtune-cli`'s own `tests/ctrlc_abort.rs` pattern and rationale: a real signal
//! can't be delivered to one `#[test]` inside a shared multi-threaded `cargo test` binary
//! without also hitting every other concurrently running test, so this has to be a real
//! subprocess. Unix-only (there is no POSIX `SIGINT`/`SIGTERM` on Windows; CI runs
//! `ubuntu-latest` only, see `AGENTS.md`).

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Spawns `bhtune-server` bound to an OS-assigned loopback port (so concurrently running
/// tests can never collide on a fixed port number), pointed at a fresh temp database and
/// temp log/data directory, and returns the child together with the port it actually bound.
async fn spawn_server() -> (Child, u16, tempfile::TempDir, tempfile::TempDir) {
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("bhtune.db");
    // Redirects the default log directory (and every other XDG-style default this process
    // would otherwise resolve) into a throwaway temp dir -- `bhtune-server` has no `--log-
    // dir`/`--config` flags of its own to override this more directly (see `main.rs`'s doc
    // comment), unlike `bhtune-cli`'s `tests/ctrlc_abort.rs`, which passes `--log-dir`
    // explicitly. Without this, logging setup would resolve the real platform default (e.g.
    // `~/.local/share/bhtune/logs`) using this test process's actual inherited `HOME`, since
    // `Command::new` inherits the parent's environment by default.
    let xdg_dir = tempfile::tempdir().unwrap();

    // Cargo preserves the hyphen literally in this variable's name (`CARGO_BIN_EXE_bhtune-
    // server`, not an underscored `CARGO_BIN_EXE_bhtune_server`) -- verified directly rather
    // than assumed, since the `[[bin]]` target name here happens to equal the package name.
    let mut child = Command::new(env!("CARGO_BIN_EXE_bhtune-server"))
        .env("BHTUNE_DB", &db_path)
        .env("BHTUNE_BIND", "127.0.0.1:0")
        .env("XDG_DATA_HOME", xdg_dir.path())
        .env("XDG_CONFIG_HOME", xdg_dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn the bhtune-server binary");

    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    // `main.rs` prints exactly this one line to stdout, immediately before it starts serving
    // -- nothing else in the process writes to stdout, so the very first line is always it.
    let listening_line = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || {
            let mut line = String::new();
            stdout.read_line(&mut line).map(|n| (n, line))
        }),
    )
    .await
    .expect("bhtune-server did not print its listening line within 10s")
    .expect("joining the stdout-reader task panicked")
    .expect("failed to read the child's stdout");
    let (bytes_read, listening_line) = listening_line;
    assert_ne!(
        bytes_read, 0,
        "bhtune-server exited before printing anything on stdout"
    );

    let port: u16 = listening_line
        .trim()
        .rsplit(':')
        .next()
        .expect("listening line has no ':'")
        .parse()
        .unwrap_or_else(|e| {
            panic!("could not parse a port out of {listening_line:?}: {e}");
        });

    (child, port, db_dir, xdg_dir)
}

/// Sends a real signal to `child`'s process ID.
///
/// SAFETY: `child.id()` is a live PID for a process this test just spawned and still owns.
fn send_signal(child: &Child, signal: libc::c_int) {
    let result = unsafe { libc::kill(child.id() as libc::pid_t, signal) };
    assert_eq!(
        result, 0,
        "failed to send signal {signal} to the child process"
    );
}

/// Issues a plain HTTP/1.1 `GET` over a raw `TcpStream` (no HTTP client dependency needed for
/// one request) and returns the full response text.
fn http_get(port: u16, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .unwrap_or_else(|e| panic!("failed to connect to 127.0.0.1:{port}: {e}"));
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

/// Waits (bounded) for `child` to exit and returns its exit code.
async fn wait_for_exit(mut child: Child, what: &str) -> i32 {
    let status = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || child.wait()),
    )
    .await
    .unwrap_or_else(|_| panic!("bhtune-server did not exit within 10s of {what}"))
    .expect("joining the wait task panicked")
    .expect("failed to wait for the bhtune-server child process");
    status.code().unwrap_or_else(|| {
        panic!("bhtune-server did not exit normally (terminated by a signal instead): {status:?}")
    })
}

#[tokio::test]
async fn serves_real_http_and_shuts_down_gracefully_on_sigterm() {
    let (child, port, _db_dir, _xdg_dir) = spawn_server().await;

    let response = http_get(port, "/api/health");
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "expected a 200 from /api/health, got: {response}"
    );
    assert!(
        response.contains(&format!(
            r#"{{"status":"ok","version":"{}"}}"#,
            env!("CARGO_PKG_VERSION")
        )),
        "expected the health body in the response, got: {response}"
    );

    // SIGTERM is what a service manager's ordinary "stop" sends (`systemctl stop`, and
    // eventually the Windows Service Control Manager -- see `server-windows-service` in
    // AGENTS.md), so this proves the deployed-as-a-service path drains cleanly, not just an
    // interactive Ctrl+C.
    send_signal(&child, libc::SIGTERM);

    let exit_code = wait_for_exit(child, "SIGTERM").await;
    assert_eq!(
        exit_code, 0,
        "expected a clean (0) exit after graceful shutdown on SIGTERM"
    );
}

#[tokio::test]
async fn shuts_down_gracefully_on_sigint() {
    let (child, port, _db_dir, _xdg_dir) = spawn_server().await;

    // Confirms the server is actually up (not just that the listening line was printed)
    // before signalling it.
    let response = http_get(port, "/api/health");
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "expected a 200 from /api/health, got: {response}"
    );

    // Ctrl+C in an interactive terminal sends exactly this signal.
    send_signal(&child, libc::SIGINT);

    let exit_code = wait_for_exit(child, "SIGINT").await;
    assert_eq!(
        exit_code, 0,
        "expected a clean (0) exit after graceful shutdown on SIGINT"
    );
}

#[tokio::test]
async fn exits_with_an_error_for_an_unparseable_bind_address() {
    let db_dir = tempfile::tempdir().unwrap();
    let xdg_dir = tempfile::tempdir().unwrap();

    let child = Command::new(env!("CARGO_BIN_EXE_bhtune-server"))
        .env("BHTUNE_DB", db_dir.path().join("bhtune.db"))
        .env("BHTUNE_BIND", "not-a-socket-address")
        .env("XDG_DATA_HOME", xdg_dir.path())
        .env("XDG_CONFIG_HOME", xdg_dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn the bhtune-server binary");

    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || child.wait_with_output()),
    )
    .await
    .expect("bhtune-server did not exit within 10s of an invalid BHTUNE_BIND")
    .expect("joining the wait_with_output task panicked")
    .expect("failed to wait for the bhtune-server child process");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected a non-zero exit for an unparseable BHTUNE_BIND, got {:?}. stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid bind address"),
        "expected the bind-address error naming the problem on stderr, got: {stderr}"
    );
}
