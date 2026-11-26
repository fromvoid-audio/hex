//! Scoped Pulse intake credential configuration (`~/.config/hex/pulse-intake.json`).
//!
//! Never prints or logs the bearer token.

use serde::Deserialize;
use std::{
    env, fmt, fs,
    io::Read,
    path::{Path, PathBuf},
};

pub const DEFAULT_INTAKE_URL: &str = "https://pulse.iocore.dev";
pub const DEFAULT_PROJECT_ID: u64 = 2;

const MAX_CONFIG_BYTES: u64 = 16 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct IntakeConfig {
    pub url: String,
    pub project_id: u64,
    token: String,
}

impl fmt::Debug for IntakeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntakeConfig")
            .field("url", &self.url)
            .field("project_id", &self.project_id)
            .field("token", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    HomeUnavailable,
    Missing(PathBuf),
    UnsafePermissions(PathBuf),
    SymlinkNotAllowed(PathBuf),
    Io(String),
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HomeUnavailable => write!(f, "home directory is unavailable"),
            Self::Missing(path) => {
                write!(f, "intake config not found at {}", path.display())
            }
            Self::UnsafePermissions(path) => write!(
                f,
                "intake config {} must have mode 600 (no group/other access)",
                path.display()
            ),
            Self::SymlinkNotAllowed(path) => {
                write!(f, "intake config {} must be a regular file", path.display())
            }
            Self::Io(message) => write!(f, "could not read intake config: {message}"),
            Self::Invalid(message) => write!(f, "invalid intake config: {message}"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Deserialize)]
struct RawIntakeConfig {
    url: Option<String>,
    project_id: Option<u64>,
    token: String,
}

/// Default config path: `~/.config/hex/pulse-intake.json`.
pub fn default_config_path() -> Result<PathBuf, ConfigError> {
    let home = env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or(ConfigError::HomeUnavailable)?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("hex")
        .join("pulse-intake.json"))
}

impl IntakeConfig {
    /// Construct an explicit validated configuration (e.g. for tests).
    pub fn new(
        url: impl Into<String>,
        project_id: u64,
        token: impl Into<String>,
    ) -> Result<Self, ConfigError> {
        let raw_url = url.into();
        let trimmed_url = raw_url.trim().trim_end_matches('/').to_owned();
        validate_url(&trimmed_url)?;
        if project_id == 0 {
            return Err(ConfigError::Invalid("project_id must be positive".into()));
        }
        let token = token.into();
        validate_token(&token)?;
        Ok(Self {
            url: trimmed_url,
            project_id,
            token,
        })
    }

    pub fn load_default() -> Result<Self, ConfigError> {
        let path = default_config_path()?;
        Self::load_from_path(&path)
    }

    pub fn load_from_path(path: &Path) -> Result<Self, ConfigError> {
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ConfigError::Missing(path.to_owned()));
            }
            Err(error) => return Err(ConfigError::Io(error.to_string())),
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(ConfigError::SymlinkNotAllowed(path.to_owned()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.mode() & 0o077 != 0 {
                return Err(ConfigError::UnsafePermissions(path.to_owned()));
            }
        }
        if meta.len() > MAX_CONFIG_BYTES {
            return Err(ConfigError::Invalid("config file exceeds 16 KiB".into()));
        }

        let mut file = fs::File::open(path).map_err(|error| ConfigError::Io(error.to_string()))?;
        let mut bytes = Vec::new();
        file.by_ref()
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| ConfigError::Io(error.to_string()))?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(ConfigError::Invalid("config file exceeds 16 KiB".into()));
        }

        let raw: RawIntakeConfig = serde_json::from_slice(&bytes)
            .map_err(|error| ConfigError::Invalid(format!("malformed json: {error}")))?;
        let url = raw.url.unwrap_or_else(|| DEFAULT_INTAKE_URL.to_owned());
        let project_id = raw.project_id.unwrap_or(DEFAULT_PROJECT_ID);
        Self::new(url, project_id, raw.token)
    }

    pub fn endpoint_url(&self) -> String {
        format!("{}/api/intake/reports", self.url)
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}

fn validate_url(url: &str) -> Result<(), ConfigError> {
    if url.is_empty() {
        return Err(ConfigError::Invalid("url must not be empty".into()));
    }
    if url
        .chars()
        .any(|c| c.is_control() || c == '"' || c == '\\' || c.is_whitespace())
    {
        return Err(ConfigError::Invalid(
            "url contains invalid characters".into(),
        ));
    }
    let allowed = url.starts_with("https://")
        || url.starts_with("http://127.0.0.1")
        || url.starts_with("http://localhost")
        || url.starts_with("http://[::1]");
    if !allowed {
        return Err(ConfigError::Invalid(
            "url must use https:// (or http:// loopback for tests)".into(),
        ));
    }
    Ok(())
}

fn validate_token(token: &str) -> Result<(), ConfigError> {
    let trimmed = token.trim();
    if trimmed.is_empty() || trimmed != token {
        return Err(ConfigError::Invalid(
            "token must be non-empty without surrounding whitespace".into(),
        ));
    }
    if token
        .chars()
        .any(|c| c.is_control() || c == '"' || c == '\\' || c.is_whitespace())
    {
        return Err(ConfigError::Invalid(
            "token contains invalid characters".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn temp_path(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "hex-report-cfg-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn loads_valid_600_config_and_redacts_debug() {
        let path = temp_path("pulse-intake.json");
        fs::write(
            &path,
            r#"{"url":"https://pulse.iocore.dev/","project_id":2,"token":"plsr_secret_abc"}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let cfg = IntakeConfig::load_from_path(&path).unwrap();
        assert_eq!(cfg.url, "https://pulse.iocore.dev");
        assert_eq!(
            cfg.endpoint_url(),
            "https://pulse.iocore.dev/api/intake/reports"
        );
        assert_eq!(cfg.project_id, 2);
        let debug = format!("{cfg:?}");
        assert!(!debug.contains("plsr_secret_abc"));
        assert!(debug.contains("[redacted]"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_permissive_config_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path("pulse-intake.json");
        fs::write(&path, r#"{"token":"plsr_secret_abc"}"#).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let err = IntakeConfig::load_from_path(&path).unwrap_err();
        assert!(matches!(err, ConfigError::UnsafePermissions(_)));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn rejects_non_https_remote_url_and_bad_tokens() {
        assert!(IntakeConfig::new("http://example.com", 2, "plsr_abc").is_err());
        assert!(IntakeConfig::new("http://127.0.0.1:9999", 2, "plsr_abc").is_ok());
        assert!(IntakeConfig::new("https://pulse.iocore.dev", 2, "bad\ntoken").is_err());
        assert!(IntakeConfig::new("https://pulse.iocore.dev", 0, "plsr_abc").is_err());
    }
}
