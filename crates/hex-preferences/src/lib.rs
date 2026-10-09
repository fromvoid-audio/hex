//! Bounded, versioned preferences independent of project documents.

use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const DOCUMENT_VERSION: u32 = 1;
const MAX_DOCUMENT_BYTES: u64 = 64 * 1024;
const MAX_PATH_BYTES: usize = 4096;
const MAX_TOTAL_PATH_BYTES: usize = 8 * 1024;
pub const MAX_USER_PLACES: usize = 14;
const TEMPORARY_ATTEMPTS: usize = 8;
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// Stable app-support location shared by development and review bundles.
pub fn browser_places_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join("hex")
        .join("browser.json")
}

/// Additional browser folders, in user-visible order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BrowserPlaces {
    additional: Vec<PathBuf>,
}

impl BrowserPlaces {
    /// Build an ordered place list, dropping duplicate exact paths and rejecting
    /// empty or excessive input rather than silently truncating saved data.
    pub fn new(paths: impl IntoIterator<Item = PathBuf>) -> Result<Self, PreferencesError> {
        let mut additional = Vec::new();
        let mut seen = HashSet::new();
        let mut total_path_bytes = 0_usize;
        for path in paths {
            if path.as_os_str().is_empty() {
                return Err(PreferencesError::EmptyPath);
            }
            validate_path_length(&path)?;
            if seen.insert(path.clone()) {
                total_path_bytes = total_path_bytes.checked_add(native_path_len(&path)).ok_or(
                    PreferencesError::DocumentTooLarge {
                        maximum: MAX_DOCUMENT_BYTES,
                    },
                )?;
                if total_path_bytes > MAX_TOTAL_PATH_BYTES {
                    return Err(PreferencesError::DocumentTooLarge {
                        maximum: MAX_DOCUMENT_BYTES,
                    });
                }
                if additional.len() == MAX_USER_PLACES {
                    return Err(PreferencesError::TooManyPlaces {
                        maximum: MAX_USER_PLACES,
                    });
                }
                additional.push(path);
            }
        }
        Ok(Self { additional })
    }

    pub fn additional(&self) -> &[PathBuf] {
        &self.additional
    }
}

#[derive(Debug)]
pub enum PreferencesError {
    Io(io::Error),
    InvalidDocument(String),
    UnsupportedPathEncoding(&'static str),
    EmptyPath,
    TooManyPlaces { maximum: usize },
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
            Self::EmptyPath => formatter.write_str("browser folder path is empty"),
            Self::TooManyPlaces { maximum } => {
                write!(formatter, "browser folder limit is {maximum}")
            }
            Self::DocumentTooLarge { maximum } => {
                write!(formatter, "preferences document exceeds {maximum} bytes")
            }
            Self::PathTooLong { maximum } => {
                write!(formatter, "browser folder path exceeds {maximum} bytes")
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

/// Load saved folders. A missing file is the default; malformed or oversized
/// files return an error and are left untouched.
pub fn load(path: &Path) -> Result<BrowserPlaces, PreferencesError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(BrowserPlaces::default());
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
    let document: Document = serde_json::from_slice(&bytes)
        .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if document.version != DOCUMENT_VERSION {
        return Err(PreferencesError::InvalidDocument(format!(
            "unsupported version {}",
            document.version
        )));
    }
    let paths = document
        .additional_places
        .into_iter()
        .map(StoredPath::into_path)
        .collect::<Result<Vec<_>, _>>()?;
    BrowserPlaces::new(paths)
}

/// Atomically publish the latest complete preferences document.
pub fn save(path: &Path, places: &BrowserPlaces) -> Result<(), PreferencesError> {
    // Refuse to replace malformed, oversized, or future-version settings files.
    // A missing file is the only empty default accepted here.
    let _ = load(path)?;
    if places.additional.len() > MAX_USER_PLACES {
        return Err(PreferencesError::TooManyPlaces {
            maximum: MAX_USER_PLACES,
        });
    }
    let mut total_path_bytes = 0_usize;
    for path in &places.additional {
        validate_path_length(path)?;
        total_path_bytes = total_path_bytes.checked_add(native_path_len(path)).ok_or(
            PreferencesError::DocumentTooLarge {
                maximum: MAX_DOCUMENT_BYTES,
            },
        )?;
        if total_path_bytes > MAX_TOTAL_PATH_BYTES {
            return Err(PreferencesError::DocumentTooLarge {
                maximum: MAX_DOCUMENT_BYTES,
            });
        }
    }
    let document = Document {
        version: DOCUMENT_VERSION,
        additional_places: places
            .additional
            .iter()
            .map(|path| StoredPath::from_path(path))
            .collect::<Result<Vec<_>, _>>()?,
    };
    let bytes = serde_json::to_vec(&document)
        .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(PreferencesError::DocumentTooLarge {
            maximum: MAX_DOCUMENT_BYTES,
        });
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let (temporary_path, mut temporary_file) = create_temporary(parent, path.file_name())?;
    let result = (|| {
        temporary_file.write_all(&bytes)?;
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

fn validate_path_length(path: &Path) -> Result<(), PreferencesError> {
    if native_path_len(path) > MAX_PATH_BYTES {
        return Err(PreferencesError::PathTooLong {
            maximum: MAX_PATH_BYTES,
        });
    }
    Ok(())
}

fn native_path_len(path: &Path) -> usize {
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
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    additional_places: Vec<StoredPath>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "encoding", content = "value", rename_all = "snake_case")]
enum StoredPath {
    Utf8(String),
    UnixBytes(Vec<u8>),
    WindowsWide(Vec<u16>),
}

impl StoredPath {
    fn from_path(path: &Path) -> Result<Self, PreferencesError> {
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

    fn into_path(self) -> Result<PathBuf, PreferencesError> {
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
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hex-preferences-test-{}-{}",
                std::process::id(),
                NEXT_TEST.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("test directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_file_is_empty_and_save_round_trips_ordered_missing_paths() {
        let directory = TestDirectory::new();
        let config = directory.0.join("browser.json");
        assert_eq!(
            load(&config).expect("missing defaults"),
            BrowserPlaces::default()
        );
        let places = BrowserPlaces::new([
            PathBuf::from("/path/that/no-longer-exists"),
            PathBuf::from("/another/offline/place"),
        ])
        .expect("places");
        save(&config, &places).expect("atomic save");
        assert_eq!(load(&config).expect("load saved"), places);
    }

    #[test]
    fn duplicate_places_keep_first_order_and_empty_paths_are_rejected() {
        let places = BrowserPlaces::new([
            PathBuf::from("/first"),
            PathBuf::from("/second"),
            PathBuf::from("/first"),
        ])
        .expect("deduplicated places");
        assert_eq!(
            places.additional(),
            &[PathBuf::from("/first"), PathBuf::from("/second")]
        );
        assert!(matches!(
            BrowserPlaces::new([PathBuf::new()]),
            Err(PreferencesError::EmptyPath)
        ));
    }

    #[test]
    fn malformed_document_is_reported_without_changing_its_bytes() {
        let directory = TestDirectory::new();
        let config = directory.0.join("browser.json");
        let corrupt = br#"{"version":99,"additional_places":[]}"#;
        fs::write(&config, corrupt).expect("write malformed document");
        assert!(matches!(
            load(&config),
            Err(PreferencesError::InvalidDocument(_))
        ));
        assert_eq!(fs::read(&config).expect("read preserved document"), corrupt);
    }

    #[test]
    fn save_refuses_to_replace_malformed_preferences() {
        let directory = TestDirectory::new();
        let config = directory.0.join("browser.json");
        let corrupt = b"not json";
        fs::write(&config, corrupt).expect("write malformed document");
        let places = BrowserPlaces::new([PathBuf::from("/new-folder")]).expect("new place");
        assert!(matches!(
            save(&config, &places),
            Err(PreferencesError::InvalidDocument(_))
        ));
        assert_eq!(fs::read(&config).expect("read preserved document"), corrupt);
    }

    #[test]
    fn oversized_paths_are_rejected_before_serialization() {
        let path = PathBuf::from(format!("/{}", "x".repeat(MAX_PATH_BYTES)));
        assert!(matches!(
            BrowserPlaces::new([path]),
            Err(PreferencesError::PathTooLong {
                maximum: MAX_PATH_BYTES
            })
        ));
    }

    #[test]
    fn aggregate_path_payload_is_bounded_before_serialization() {
        let paths = (0..5)
            .map(|index| PathBuf::from(format!("/{index}-{}", "x".repeat(MAX_PATH_BYTES / 2))));
        assert!(matches!(
            BrowserPlaces::new(paths),
            Err(PreferencesError::DocumentTooLarge { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_unix_path_bytes_round_trip() {
        use std::os::unix::ffi::OsStringExt;
        let directory = TestDirectory::new();
        let config = directory.0.join("browser.json");
        let path = PathBuf::from(OsString::from_vec(b"/tmp/folder-\xff".to_vec()));
        let places = BrowserPlaces::new([path]).expect("native path");
        save(&config, &places).expect("save native path");
        assert_eq!(load(&config).expect("load native path"), places);
    }

    #[test]
    fn excess_paths_are_rejected_without_truncation() {
        let paths = (0..=MAX_USER_PLACES).map(|index| PathBuf::from(format!("/place-{index}")));
        assert!(matches!(
            BrowserPlaces::new(paths),
            Err(PreferencesError::TooManyPlaces {
                maximum: MAX_USER_PLACES
            })
        ));
    }
}
