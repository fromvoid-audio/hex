//! Injectable report intake transport and `/usr/bin/curl` implementation.
//!
//! `CurlTransport` passes the authorization header strictly over stdin via
//! `--config -` and writes the JSON request body to a private `0600` temp file,
//! never exposing bearer tokens in `argv`, environment variables, or logs.

use crate::{
    config::IntakeConfig,
    model::{IntakeReceipt, IntakeResponseEnvelope, ReportEnvelope},
};
use std::{
    env, fmt,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

const DEFAULT_TIMEOUT_SECS: u64 = 15;
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 5;
const DEFAULT_CURL_PATH: &str = "/usr/bin/curl";

static NEXT_TMP_BODY: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    PayloadTooLarge(String),
    Network(String),
    HttpStatus { status: u16, body: String },
    InvalidResponse(String),
    Io(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge(msg) => write!(f, "{msg}"),
            Self::Network(msg) => write!(f, "network error: {msg}"),
            Self::HttpStatus { status, body } => {
                if body.is_empty() {
                    write!(f, "pulse intake returned http {status}")
                } else {
                    write!(f, "pulse intake returned http {status}: {body}")
                }
            }
            Self::InvalidResponse(msg) => write!(f, "invalid pulse intake response: {msg}"),
            Self::Io(msg) => write!(f, "transport io error: {msg}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// Injectable transport contract for submitting one report envelope.
pub trait ReportTransport: Send + Sync {
    fn submit(&self, envelope: &ReportEnvelope) -> Result<IntakeReceipt, TransportError>;
}

/// Production transport backed by macOS/Unix `/usr/bin/curl` using `--config -` on stdin.
#[derive(Clone, Debug)]
pub struct CurlTransport {
    config: IntakeConfig,
    curl_path: PathBuf,
    timeout_secs: u64,
}

impl CurlTransport {
    pub fn new(config: IntakeConfig) -> Self {
        Self {
            config,
            curl_path: PathBuf::from(DEFAULT_CURL_PATH),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }

    pub fn with_timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs.max(1);
        self
    }

    pub fn with_curl_path(mut self, path: PathBuf) -> Self {
        self.curl_path = path;
        self
    }
}

struct TempBodyFile(PathBuf);

impl TempBodyFile {
    fn create(bytes: &[u8]) -> Result<Self, TransportError> {
        let path = env::temp_dir().join(format!(
            "hex-report-body-{}-{}.json",
            std::process::id(),
            NEXT_TMP_BODY.fetch_add(1, Ordering::Relaxed)
        ));
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts
            .open(&path)
            .map_err(|error| TransportError::Io(error.to_string()))?;
        file.write_all(bytes)
            .and_then(|()| file.flush())
            .map_err(|error| TransportError::Io(error.to_string()))?;
        Ok(Self(path))
    }
}

impl Drop for TempBodyFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn escape_curl_config_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

impl ReportTransport for CurlTransport {
    fn submit(&self, envelope: &ReportEnvelope) -> Result<IntakeReceipt, TransportError> {
        let body_bytes = envelope
            .to_wire_json()
            .map_err(TransportError::PayloadTooLarge)?;
        let body_file = TempBodyFile::create(&body_bytes)?;
        let body_path_str = body_file
            .0
            .to_str()
            .ok_or_else(|| TransportError::Io("temp body path is not valid utf-8".into()))?;

        let endpoint = self.config.endpoint_url();
        let connect_timeout = self.timeout_secs.min(DEFAULT_CONNECT_TIMEOUT_SECS);
        let curl_config = format!(
            "silent\n\
             show-error\n\
             max-time = {}\n\
             connect-timeout = {}\n\
             request = \"POST\"\n\
             url = \"{}\"\n\
             header = \"Content-Type: application/json\"\n\
             header = \"Accept: application/json\"\n\
             header = \"Authorization: Bearer {}\"\n\
             data-binary = \"@{}\"\n\
             write-out = \"\\n%{{http_code}}\"\n",
            self.timeout_secs,
            connect_timeout,
            escape_curl_config_value(&endpoint),
            escape_curl_config_value(self.config.token()),
            escape_curl_config_value(body_path_str),
        );

        let mut child = Command::new(&self.curl_path)
            .args(["--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| TransportError::Io(format!("could not spawn curl: {error}")))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(curl_config.as_bytes()).map_err(|error| {
                TransportError::Io(format!("could not write curl config: {error}"))
            })?;
        }

        let output = child
            .wait_with_output()
            .map_err(|error| TransportError::Io(format!("could not await curl: {error}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let sanitized = stderr
                .replace(self.config.token(), "[redacted]")
                .trim()
                .chars()
                .take(256)
                .collect::<String>();
            return Err(TransportError::Network(if sanitized.is_empty() {
                format!("curl exited with {}", output.status)
            } else {
                sanitized
            }));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let (body_part, status_part) = stdout
            .rsplit_once('\n')
            .ok_or_else(|| TransportError::InvalidResponse("missing http status trailer".into()))?;
        let status: u16 = status_part
            .trim()
            .parse()
            .map_err(|_| TransportError::InvalidResponse("malformed http status code".into()))?;

        if status != 200 && status != 201 {
            let snippet = body_part
                .replace(self.config.token(), "[redacted]")
                .trim()
                .chars()
                .take(256)
                .collect();
            return Err(TransportError::HttpStatus {
                status,
                body: snippet,
            });
        }

        let parsed: IntakeResponseEnvelope = serde_json::from_str(body_part).map_err(|error| {
            TransportError::InvalidResponse(format!("could not parse receipt json: {error}"))
        })?;
        Ok(parsed.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AudioRuntimeSnapshot, ReportKind, RuntimeSnapshot};
    use std::{
        io::{BufRead, BufReader, Read},
        net::TcpListener,
        thread,
    };

    struct CapturedRequest {
        request_line: String,
        auth_header: Option<String>,
        body: String,
    }

    fn spawn_mock_server(
        status_line: &'static str,
        response_body: String,
    ) -> (String, thread::JoinHandle<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut auth_header = None;
            let mut content_length = 0usize;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                let trimmed = header.trim();
                if trimmed.is_empty() {
                    break;
                }
                if let Some((name, value)) = trimmed.split_once(':') {
                    if name.eq_ignore_ascii_case("authorization") {
                        auth_header = Some(value.trim().to_owned());
                    } else if name.eq_ignore_ascii_case("content-length") {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body_buf = vec![0u8; content_length];
            reader.read_exact(&mut body_buf).unwrap();
            let body = String::from_utf8(body_buf).unwrap();

            let response = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();

            CapturedRequest {
                request_line: request_line.trim().to_owned(),
                auth_header,
                body,
            }
        });
        (format!("http://{addr}"), handle)
    }

    fn sample_envelope() -> ReportEnvelope {
        ReportEnvelope::with_request_id(
            "33333333-3333-4333-8333-333333333333".into(),
            2,
            ReportKind::Bug,
            "reproducible crash on bounce".into(),
            RuntimeSnapshot {
                app_version: "0.1.0".into(),
                source_sha: "abc12345".into(),
                tree_state: "clean".into(),
                build_label: "0.1.0 · abc12345".into(),
                platform: "macos-aarch64".into(),
                reported_at_unix_nanos: 123456,
                view_mode: "arrangement".into(),
                status: "ready".into(),
                last_error: None,
                audio: AudioRuntimeSnapshot {
                    sample_rate: 48_000,
                    mode: "resident".into(),
                    ..Default::default()
                },
                loaded_plugins: vec![],
            },
            None,
        )
    }

    #[test]
    fn curl_transport_posts_envelope_with_bearer_header_via_stdin_config() {
        let response = r#"{"data":{"report_id":42,"project_id":2,"request_id":"33333333-3333-4333-8333-333333333333","status":"new","deduplicated":false}}"#.to_owned();
        let (base_url, server) = spawn_mock_server("201 Created", response);
        let cfg = IntakeConfig::new(base_url, 2, "plsr_unit_test_secret").unwrap();
        let transport = CurlTransport::new(cfg).with_timeout_secs(5);
        let env = sample_envelope();

        let receipt = transport.submit(&env).unwrap();
        assert_eq!(receipt.report_id, 42);
        assert_eq!(receipt.project_id, 2);
        assert_eq!(receipt.request_id, env.request_id);
        assert!(!receipt.deduplicated);

        let captured = server.join().unwrap();
        assert_eq!(captured.request_line, "POST /api/intake/reports HTTP/1.1");
        assert_eq!(
            captured.auth_header.as_deref(),
            Some("Bearer plsr_unit_test_secret")
        );
        let decoded: ReportEnvelope = serde_json::from_str(&captured.body).unwrap();
        assert_eq!(decoded, env);
    }

    #[test]
    fn curl_transport_surfaces_http_errors_without_leaking_token() {
        let (base_url, server) = spawn_mock_server(
            "401 Unauthorized",
            r#"{"error":"invalid token plsr_unit_test_secret"}"#.to_owned(),
        );
        let cfg = IntakeConfig::new(base_url, 2, "plsr_unit_test_secret").unwrap();
        let transport = CurlTransport::new(cfg).with_timeout_secs(5);
        let err = transport.submit(&sample_envelope()).unwrap_err();
        let _ = server.join().unwrap();
        match &err {
            TransportError::HttpStatus { status, body } => {
                assert_eq!(*status, 401);
                assert!(!body.contains("plsr_unit_test_secret"));
            }
            other => panic!("expected HttpStatus, got {other:?}"),
        }
        assert!(!err.to_string().contains("plsr_unit_test_secret"));
    }
}
