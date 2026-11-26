//! Shared bounded versioned JSON storage, atomic publication, native path
//! encoding, and platform application-support directory resolution.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) const DOCUMENT_VERSION: u32 = 1;
pub(crate) const MAX_DOCUMENT_BYTES: u64 = 64 * 1024;
pub(crate) const MAX_PATH_BYTES: usize = 4096;
pub(crate) const MAX_TOTAL_PATH_BYTES: usize = 8 * 1024;
const TEMPORARY_ATTEMPTS: usize = 8;
pub(crate) static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// Centralized application support directory derived from an explicit home path.
///
/// Matches `hex-diagnostics` platform conventions:
/// - macOS: `<home>/Library/Application Support/hex`
/// - Windows: `<home>/AppData/Roaming/hex`
/// - Linux / other Unix: `<home>/.local/share/hex`
pub fn app_support_dir_from(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support").join("hex")
    } else if cfg!(windows) {
        home.join("AppData").join("Roaming").join("hex")
    } else {
        home.join(".local").join("share").join("hex")
    }
}

/// Resolve the application support directory from standard platform environment variables.
pub fn app_support_dir() -> Result<PathBuf, String> {
    app_support_dir_with_env(
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
        std::env::var_os("APPDATA"),
        std::env::var_os("XDG_DATA_HOME"),
    )
}

pub(crate) fn app_support_dir_with_env(
    home: Option<OsString>,
    user_profile: Option<OsString>,
    app_data: Option<OsString>,
    xdg_data_home: Option<OsString>,
) -> Result<PathBuf, String> {
    if cfg!(windows) {
        if let Some(appdata) = app_data.filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(appdata).join("hex"));
        }
        if let Some(profile) = user_profile.filter(|v| !v.is_empty()) {
            return Ok(app_support_dir_from(Path::new(&profile)));
        }
    } else if !cfg!(target_os = "macos")
        && let Some(xdg) = xdg_data_home
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    {
        return Ok(xdg.join("hex"));
    }
    let home = home
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "home directory is unavailable".to_owned())?;
    Ok(app_support_dir_from(Path::new(&home)))
}

#[derive(Debug)]
pub enum PreferencesError {
    Io(io::Error),
    InvalidDocument(String),
    UnsupportedPathEncoding(&'static str),
    EmptyPath,
    TooManyPlaces { maximum: usize },
    TooManyRecentProjects { maximum: usize },
    DocumentTooLarge { maximum: u64 },
    PathTooLong { maximum: usize },
    TemporaryNameExhausted,
}

impl std::fmt::Display for PreferencesError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "preferences file error: {error}"),
            Self::InvalidDocument(reason) => write!(formatter, "invalid preferences: {reason}"),
            Self::UnsupportedPathEncoding(encoding) => {
                write!(
                    formatter,
                    "saved path encoding is unsupported on this platform: {encoding}"
                )
            }
            Self::EmptyPath => formatter.write_str("path is empty"),
            Self::TooManyPlaces { maximum } => {
                write!(formatter, "browser folder limit is {maximum}")
            }
            Self::TooManyRecentProjects { maximum } => {
                write!(formatter, "recent project limit is {maximum}")
            }
            Self::DocumentTooLarge { maximum } => {
                write!(formatter, "preferences document exceeds {maximum} bytes")
            }
            Self::PathTooLong { maximum } => {
                write!(formatter, "path exceeds {maximum} bytes")
            }
            Self::TemporaryNameExhausted => {
                formatter.write_str("could not reserve a temporary preferences file")
            }
        }
    }
}

impl std::error::Error for PreferencesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for PreferencesError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub(crate) trait VersionedDocument {
    fn version(&self) -> u32;
}

pub(crate) fn decode_versioned<D>(bytes: &[u8]) -> Result<D, PreferencesError>
where
    D: DeserializeOwned + VersionedDocument,
{
    let document: D = serde_json::from_slice(bytes)
        .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if document.version() != DOCUMENT_VERSION {
        return Err(PreferencesError::InvalidDocument(format!(
            "unsupported version {}",
            document.version()
        )));
    }
    Ok(document)
}

pub(crate) fn load_versioned<D, T>(
    path: &Path,
    default: impl FnOnce() -> T,
    parse: impl FnOnce(D) -> Result<T, PreferencesError>,
) -> Result<T, PreferencesError>
where
    D: DeserializeOwned + VersionedDocument,
{
    let Some(bytes) = read_document(path)? else {
        return Ok(default());
    };
    parse(decode_versioned(&bytes)?)
}

pub(crate) fn save_versioned<D>(
    path: &Path,
    verify_existing: impl FnOnce(&Path) -> Result<(), PreferencesError>,
    document: &D,
) -> Result<(), PreferencesError>
where
    D: Serialize,
{
    verify_existing(path)?;
    let bytes = serde_json::to_vec(document)
        .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(PreferencesError::DocumentTooLarge {
            maximum: MAX_DOCUMENT_BYTES,
        });
    }
    atomic_save(path, &bytes)
}

pub(crate) fn read_document(path: &Path) -> Result<Option<Vec<u8>>, PreferencesError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            #[cfg(windows)]
            if let Some(parent) = path.parent() {
                // Windows maps ERROR_PATH_NOT_FOUND to NotFound even when an
                // ancestor path component is a regular file instead of a directory.
                for ancestor in parent.ancestors() {
                    if ancestor.as_os_str().is_empty() {
                        break;
                    }
                    match fs::metadata(ancestor) {
                        Ok(meta) if meta.is_dir() => break,
                        Ok(_) => {
                            return Err(io::Error::new(io::ErrorKind::NotADirectory, error).into());
                        }
                        Err(ancestor_error) if ancestor_error.kind() == io::ErrorKind::NotFound => {
                        }
                        Err(ancestor_error) => return Err(ancestor_error.into()),
                    }
                }
            }
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    let size = file.metadata()?.len();
    if size > MAX_DOCUMENT_BYTES {
        return Err(PreferencesError::DocumentTooLarge {
            maximum: MAX_DOCUMENT_BYTES,
        });
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(MAX_DOCUMENT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(PreferencesError::DocumentTooLarge {
            maximum: MAX_DOCUMENT_BYTES,
        });
    }
    Ok(Some(bytes))
}

pub(crate) fn atomic_save(path: &Path, bytes: &[u8]) -> Result<(), PreferencesError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let (temporary_path, mut temporary_file) = create_temporary(parent, path.file_name())?;
    let result = (|| {
        temporary_file.write_all(bytes)?;
        temporary_file.sync_all()?;
        drop(temporary_file);
        fs::rename(&temporary_path, path)?;
        sync_directory(parent)?;
        Ok::<(), io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result.map_err(PreferencesError::Io)
}

pub(crate) fn validate_saved_path(path: &Path) -> Result<(), PreferencesError> {
    if path.as_os_str().is_empty() {
        return Err(PreferencesError::EmptyPath);
    }
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        return Err(PreferencesError::InvalidDocument(
            "path contains a nul byte".into(),
        ));
    }
    validate_path_length(path)
}

pub(crate) fn validate_path_length(path: &Path) -> Result<(), PreferencesError> {
    if native_path_len(path) > MAX_PATH_BYTES {
        return Err(PreferencesError::PathTooLong {
            maximum: MAX_PATH_BYTES,
        });
    }
    Ok(())
}

pub(crate) fn native_path_len(path: &Path) -> usize {
    if let Some(text) = path.to_str() {
        return text.len();
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().len()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().count().saturating_mul(2)
    }
    #[cfg(not(any(unix, windows)))]
    {
        usize::MAX
    }
}

pub(crate) fn total_native_path_bytes(paths: &[PathBuf]) -> usize {
    paths.iter().map(|path| native_path_len(path)).sum()
}

fn create_temporary(
    parent: &Path,
    target_name: Option<&std::ffi::OsStr>,
) -> Result<(PathBuf, File), PreferencesError> {
    let target_name = target_name.unwrap_or_else(|| std::ffi::OsStr::new("preferences"));
    for _ in 0..TEMPORARY_ATTEMPTS {
        let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
        let mut name = OsString::from(".");
        name.push(target_name);
        name.push(format!(".hex-pending-{}-{sequence}", std::process::id()));
        let temporary_path = parent.join(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((temporary_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(PreferencesError::TemporaryNameExhausted)
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> io::Result<()> {
    File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> io::Result<()> {
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "encoding", content = "value", rename_all = "snake_case")]
pub(crate) enum StoredPath {
    Utf8(String),
    UnixBytes(Vec<u8>),
    WindowsWide(Vec<u16>),
}

impl StoredPath {
    pub(crate) fn from_path(path: &Path) -> Result<Self, PreferencesError> {
        if let Some(text) = path.to_str() {
            return Ok(Self::Utf8(text.to_owned()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Ok(Self::UnixBytes(path.as_os_str().as_bytes().to_vec()))
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            Ok(Self::WindowsWide(path.as_os_str().encode_wide().collect()))
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(PreferencesError::UnsupportedPathEncoding("native"))
        }
    }

    pub(crate) fn into_path(self) -> Result<PathBuf, PreferencesError> {
        match self {
            Self::Utf8(text) => Ok(PathBuf::from(text)),
            Self::UnixBytes(bytes) => {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;
                    Ok(PathBuf::from(OsString::from_vec(bytes)))
                }
                #[cfg(not(unix))]
                {
                    let _ = bytes;
                    Err(PreferencesError::UnsupportedPathEncoding("unix_bytes"))
                }
            }
            Self::WindowsWide(units) => {
                #[cfg(windows)]
                {
                    use std::os::windows::ffi::OsStringExt;
                    Ok(PathBuf::from(OsString::from_wide(&units)))
                }
                #[cfg(not(windows))]
                {
                    let _ = units;
                    Err(PreferencesError::UnsupportedPathEncoding("windows_wide"))
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) struct TestDirectory(pub(crate) PathBuf);

#[cfg(test)]
impl TestDirectory {
    pub(crate) fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("test directory");
        Self(path)
    }
}

#[cfg(test)]
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
