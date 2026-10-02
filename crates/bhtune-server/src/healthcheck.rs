use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use bhtune_runtime::config::{self, BhtuneConfig};

const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RESPONSE_BYTES: usize = 8 * 1024;

pub(super) fn run(config_path: Option<&Path>) -> anyhow::Result<()> {
    let deadline = deadline_after(HEALTHCHECK_TIMEOUT)?;
    let loaded_config = config::load_config_store(config_path)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let address = probe_address(std::env::var("BHTUNE_BIND").ok(), &loaded_config.config)?;
    probe_until(address, deadline)
}

fn probe_address(
    bind_override: Option<String>,
    config: &BhtuneConfig,
) -> anyhow::Result<SocketAddr> {
    let bind_address = config::resolve_bind_addr(bind_override, config);
    loopback_addr(&bind_address)
}

fn loopback_addr(bind_address: &str) -> anyhow::Result<SocketAddr> {
    let bind_address: SocketAddr = bind_address
        .parse()
        .with_context(|| format!("invalid bind address '{bind_address}'"))?;
    if bind_address.port() == 0 {
        bail!("healthcheck cannot probe a bind address with port 0");
    }
    let loopback = match bind_address.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
    };
    Ok(SocketAddr::new(loopback, bind_address.port()))
}

fn deadline_after(timeout: Duration) -> anyhow::Result<Instant> {
    Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| anyhow::anyhow!("healthcheck timeout is out of range"))
}

fn probe_until(address: SocketAddr, deadline: Instant) -> anyhow::Result<()> {
    let mut stream = TcpStream::connect_timeout(&address, remaining_timeout(deadline)?)
        .with_context(|| format!("healthcheck could not connect to {address}"))?;
    remaining_timeout(deadline)?;

    let host = address.to_string();
    let request = format!(
        "GET /api/health HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    stream
        .set_write_timeout(Some(remaining_timeout(deadline)?))
        .context("could not set the healthcheck write timeout")?;
    stream
        .write_all(request.as_bytes())
        .context("could not write the healthcheck request")?;
    remaining_timeout(deadline)?;

    let response = read_response(&mut stream, deadline)?;
    validate_response(&response)?;
    remaining_timeout(deadline)?;
    Ok(())
}

fn remaining_timeout(deadline: Instant) -> anyhow::Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        bail!("healthcheck exceeded its overall timeout");
    }
    Ok(remaining)
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

struct ParsedHeaders {
    status: u16,
    content_length: usize,
}

fn read_response(stream: &mut TcpStream, deadline: Instant) -> anyhow::Result<HttpResponse> {
    let mut response = Vec::with_capacity(512);
    let mut buffer = [0_u8; 512];

    loop {
        stream
            .set_read_timeout(Some(remaining_timeout(deadline)?))
            .context("could not set the healthcheck read timeout")?;
        let bytes_read = read_chunk(&mut buffer, |buffer| stream.read(buffer))?;
        remaining_timeout(deadline)?;

        if response.len().saturating_add(bytes_read) > MAX_RESPONSE_BYTES {
            bail!("healthcheck response exceeded the size limit");
        }
        response.extend_from_slice(&buffer[..bytes_read]);

        if let Some(headers_end) = find_header_end(&response) {
            let headers = parse_headers(&response[..headers_end])?;
            let body_start = headers_end + 4;
            let response_end = body_start
                .checked_add(headers.content_length)
                .ok_or_else(|| anyhow::anyhow!("healthcheck response length overflow"))?;
            if response_end > MAX_RESPONSE_BYTES {
                bail!("healthcheck response exceeded the size limit");
            }
            if response.len() > response_end {
                bail!("healthcheck response exceeded its Content-Length");
            }
            if response.len() == response_end {
                return Ok(HttpResponse {
                    status: headers.status,
                    body: response[body_start..response_end].to_vec(),
                });
            }
        }
    }
}

fn read_chunk(
    buffer: &mut [u8],
    mut read: impl FnMut(&mut [u8]) -> std::io::Result<usize>,
) -> anyhow::Result<usize> {
    loop {
        match read(buffer) {
            Ok(0) => bail!("server closed before completing the healthcheck response"),
            Ok(bytes_read) => return Ok(bytes_read),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).context("could not read the healthcheck response"),
        }
    }
}

fn find_header_end(response: &[u8]) -> Option<usize> {
    response.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_headers(headers: &[u8]) -> anyhow::Result<ParsedHeaders> {
    let headers = std::str::from_utf8(headers).context("HTTP headers were not valid UTF-8")?;
    let mut lines = headers.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut status_fields = status_line.split_ascii_whitespace();
    if status_fields.next() != Some("HTTP/1.1") {
        bail!("healthcheck response was not HTTP/1.1");
    }
    let status = status_fields
        .next()
        .ok_or_else(|| anyhow::anyhow!("healthcheck response omitted its HTTP status"))?
        .parse::<u16>()
        .context("healthcheck response contained an invalid HTTP status")?;

    let mut content_length = None;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("healthcheck response contained a malformed header"))?;
        if name.eq_ignore_ascii_case("transfer-encoding") {
            bail!("healthcheck response used unsupported transfer encoding");
        }
        if name.eq_ignore_ascii_case("content-length")
            && content_length.replace(value.trim()).is_some()
        {
            bail!("healthcheck response contained duplicate Content-Length headers");
        }
    }

    let content_length = content_length
        .ok_or_else(|| anyhow::anyhow!("healthcheck response omitted Content-Length"))?
        .parse::<usize>()
        .context("healthcheck response contained an invalid Content-Length")?;
    Ok(ParsedHeaders {
        status,
        content_length,
    })
}

fn validate_response(response: &HttpResponse) -> anyhow::Result<()> {
    if response.status != 200 {
        bail!(
            "healthcheck endpoint returned HTTP status {}",
            response.status
        );
    }
    let body: serde_json::Value = serde_json::from_slice(&response.body)
        .context("healthcheck response was not valid JSON")?;
    if body.get("status").and_then(serde_json::Value::as_str) != Some("ok")
        || body.get("version").and_then(serde_json::Value::as_str)
            != Some(env!("CARGO_PKG_VERSION"))
    {
        bail!("healthcheck endpoint reported an unhealthy or unexpected response");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn healthcheck_uses_the_effective_bind_port_and_loopback_address() {
        let config = BhtuneConfig {
            bind: Some("0.0.0.0:8788".to_string()),
            ..BhtuneConfig::default()
        };
        assert_eq!(
            probe_address(Some("0.0.0.0:8789".to_string()), &config).unwrap(),
            "127.0.0.1:8789".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            probe_address(None, &config).unwrap(),
            "127.0.0.1:8788".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            probe_address(None, &BhtuneConfig::default()).unwrap(),
            "127.0.0.1:8787".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            loopback_addr("[::]:8787").unwrap(),
            "[::1]:8787".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn interrupted_reads_are_retried() {
        let mut attempts = 0;
        let mut buffer = [0_u8; 1];
        let bytes_read = read_chunk(&mut buffer, |buffer| {
            attempts += 1;
            if attempts == 1 {
                Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
            } else {
                buffer[0] = b'x';
                Ok(1)
            }
        })
        .unwrap();
        assert_eq!(bytes_read, 1);
        assert_eq!(buffer, [b'x']);
        assert_eq!(attempts, 2);
    }

    #[test]
    fn non_interrupted_read_errors_are_reported() {
        let mut buffer = [0_u8; 1];
        let error = read_chunk(&mut buffer, |_| {
            Err(std::io::Error::other("connection failed"))
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("could not read the healthcheck response")
        );
    }

    #[test]
    fn invalid_or_ephemeral_bind_addresses_are_rejected() {
        assert!(loopback_addr("not-an-address").is_err());
        assert!(loopback_addr("127.0.0.1:0").is_err());
    }

    #[test]
    fn production_timeout_is_three_seconds() {
        assert_eq!(HEALTHCHECK_TIMEOUT, Duration::from_secs(3));
    }

    #[test]
    fn expired_deadlines_fail_without_io() {
        let deadline = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .expect("a one-second-past instant should be representable");
        assert!(remaining_timeout(deadline).is_err());
    }

    #[test]
    fn timeout_overflow_is_rejected() {
        let error = deadline_after(Duration::MAX).unwrap_err();
        assert!(error.to_string().contains("out of range"));
    }

    #[test]
    fn healthy_endpoint_receives_http_11_get_on_the_health_path() {
        let body = health_body();
        let (address, server) = spawn_response(http_response(200, &body));

        probe_for(address, HEALTHCHECK_TIMEOUT).unwrap();

        let request = server.join().unwrap();
        assert!(request.starts_with("GET /api/health HTTP/1.1\r\n"));
        assert!(request.contains(&format!("Host: 127.0.0.1:{}", address.port())));
    }

    #[test]
    fn refused_connections_fail() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);

        assert!(probe_for(address, HEALTHCHECK_TIMEOUT).is_err());
    }

    #[test]
    fn non_200_and_unhealthy_json_responses_fail() {
        for status in [404, 503, 418] {
            let (address, server) = spawn_response(http_response(status, &health_body()));
            assert!(probe_for(address, HEALTHCHECK_TIMEOUT).is_err());
            server.join().unwrap();
        }
        assert!(
            validate_response(&HttpResponse {
                status: 200,
                body: br#"{"status":"ok","version":"0.0.0"}"#.to_vec(),
            })
            .is_err()
        );
        assert!(
            validate_response(&HttpResponse {
                status: 200,
                body: br#"{"status":"unhealthy","version":"0.1.0"}"#.to_vec(),
            })
            .is_err()
        );
        assert!(
            validate_response(&HttpResponse {
                status: 200,
                body: b"not json".to_vec(),
            })
            .is_err()
        );
    }

    #[test]
    fn malformed_headers_are_rejected() {
        for headers in [
            &b"\xff"[..],
            b"HTTP/1.0 200 OK",
            b"HTTP/1.1",
            b"HTTP/1.1 invalid",
            b"HTTP/1.1 200 OK\r\nmalformed",
            b"HTTP/1.1 200 OK\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked",
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nContent-Length: 0",
            b"HTTP/1.1 200 OK",
            b"HTTP/1.1 200 OK\r\nContent-Length: invalid",
        ] {
            assert!(parse_headers(headers).is_err(), "accepted {headers:?}");
        }
    }

    #[test]
    fn truncated_and_excessive_responses_are_rejected() {
        let (address, server) =
            spawn_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nx".to_vec());
        assert!(probe_for(address, HEALTHCHECK_TIMEOUT).is_err());
        server.join().unwrap();

        let (address, server) =
            spawn_response(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nxy".to_vec());
        assert!(probe_for(address, HEALTHCHECK_TIMEOUT).is_err());
        server.join().unwrap();

        let (address, server) = spawn_response(
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", usize::MAX).into_bytes(),
        );
        assert!(probe_for(address, HEALTHCHECK_TIMEOUT).is_err());
        server.join().unwrap();

        let (address, server) = spawn_response(
            format!("HTTP/1.1 200 OK\r\nContent-Length: {MAX_RESPONSE_BYTES}\r\n\r\n").into_bytes(),
        );
        assert!(probe_for(address, HEALTHCHECK_TIMEOUT).is_err());
        server.join().unwrap();
    }

    #[test]
    fn oversized_header_blocks_are_rejected() {
        let (address, server) = spawn_response(vec![b'x'; MAX_RESPONSE_BYTES + 1]);
        assert!(probe_for(address, HEALTHCHECK_TIMEOUT).is_err());
        server.join().unwrap();
    }

    #[test]
    fn a_slow_response_cannot_extend_the_overall_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 256];
            let _ = stream.read(&mut request).unwrap();
            for _ in 0..12 {
                thread::sleep(Duration::from_millis(100));
                if stream.write_all(b"x").is_err() {
                    break;
                }
            }
        });

        let start = Instant::now();
        assert!(probe_for(address, Duration::from_millis(400)).is_err());
        assert!(
            start.elapsed() < Duration::from_millis(800),
            "probe exceeded its single 400 ms test budget"
        );
        server.join().unwrap();
    }

    fn health_body() -> String {
        format!(
            r#"{{"status":"ok","version":"{}"}}"#,
            env!("CARGO_PKG_VERSION")
        )
    }

    fn probe_for(address: SocketAddr, timeout: Duration) -> anyhow::Result<()> {
        probe_until(address, deadline_after(timeout)?)
    }

    fn http_response(status: u16, body: &str) -> Vec<u8> {
        let reason = match status {
            200 => "OK",
            404 => "Not Found",
            503 => "Service Unavailable",
            _ => "Response",
        };
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn spawn_response(response: Vec<u8>) -> (SocketAddr, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 256];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let bytes_read = stream.read(&mut buffer).unwrap();
                assert_ne!(
                    bytes_read, 0,
                    "healthcheck request should include its headers"
                );
                request.extend_from_slice(&buffer[..bytes_read]);
            }
            let request = String::from_utf8(request).unwrap();
            stream.write_all(&response).unwrap();
            request
        });
        (address, server)
    }
}
