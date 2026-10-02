#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests may use unwrap, expect, and panic"
)]

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct SideEffectPaths {
    database: PathBuf,
    data_home: PathBuf,
    config_home: PathBuf,
}

impl Drop for SideEffectPaths {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.database);
        let _ = fs::remove_dir_all(&self.data_home);
        let _ = fs::remove_dir_all(&self.config_home);
    }
}

#[test]
fn healthcheck_probes_http_without_bootstrap_or_persistent_side_effects() {
    let executable_dir = std::env::current_exe()
        .expect("test executable path should be available")
        .parent()
        .expect("test executable should have a parent directory")
        .to_path_buf();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_nanos();
    let stem = format!("bhtune-healthcheck-{}-{nonce}", std::process::id());
    let paths = SideEffectPaths {
        database: executable_dir.join(format!("{stem}.db")),
        data_home: executable_dir.join(format!("{stem}-data")),
        config_home: executable_dir.join(format!("{stem}-config")),
    };
    assert!(!paths.database.exists());
    assert!(!paths.data_home.exists());
    assert!(!paths.config_home.exists());
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&paths.database)
        .expect("create an empty database sentinel");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test health endpoint");
    let address = listener.local_addr().expect("read test endpoint address");
    listener
        .set_nonblocking(true)
        .expect("make the test listener nonblocking");
    let server = thread::spawn(move || serve_health_endpoint(listener));

    let output = Command::new(env!("CARGO_BIN_EXE_bhtune-server"))
        .arg("healthcheck")
        .env("BHTUNE_BIND", address.to_string())
        .env("BHTUNE_DB", &paths.database)
        .env("BHTUNE_SERVER_MODE", "full")
        .env("XDG_CONFIG_HOME", &paths.config_home)
        .env("XDG_DATA_HOME", &paths.data_home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run bhtune-server healthcheck");

    assert!(
        output.status.success(),
        "healthcheck failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "healthcheck must not write to stdout"
    );
    assert!(output.stderr.is_empty());

    let request = server.join().expect("health endpoint thread should finish");
    assert!(request.starts_with("GET /api/health HTTP/1.1\r\n"));
    assert_eq!(
        fs::metadata(&paths.database)
            .expect("database sentinel should still exist")
            .len(),
        0,
        "healthcheck must not open the database or run migrations"
    );
    assert!(
        !paths.data_home.exists(),
        "healthcheck must not create log or data directories"
    );
    assert!(
        !paths.config_home.exists(),
        "healthcheck must not create configuration directories"
    );
}

fn serve_health_endpoint(listener: TcpListener) -> String {
    let deadline = Instant::now() + Duration::from_secs(2);
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(connection) => break connection,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("healthcheck did not connect to the test endpoint: {error}"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set test endpoint read timeout");
    let mut request = Vec::new();
    let mut buffer = [0_u8; 256];
    loop {
        let bytes_read = stream.read(&mut buffer).expect("read healthcheck request");
        if bytes_read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..bytes_read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let request = String::from_utf8(request).expect("HTTP request should be UTF-8");
    let body = format!(
        r#"{{"status":"ok","version":"{}"}}"#,
        env!("CARGO_PKG_VERSION")
    );
    let status = if request.starts_with("GET /api/health HTTP/1.1\r\n") {
        "200 OK"
    } else {
        "404 Not Found"
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .expect("write test health response");
    request
}
