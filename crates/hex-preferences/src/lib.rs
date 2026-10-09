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
pub const MAX_RECENT_PROJECTS: usize = 10;
const TEMPORARY_ATTEMPTS: usize = 8;
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// Stable app-support location shared by development and review bundles.
pub fn browser_places_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join("hex")
        .join("browser.json")
}

/// Stable app-support location for the independent recent-project history.
pub fn recent_projects_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join("hex")
        .join("recent-projects.json")
}

/// Stable app-support location for the independent four-color theme.
pub fn theme_preferences_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join("hex")
        .join("theme.json")
}

/// The four editable colors of the application palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemePreferences {
    pub background: RgbColor,
    pub text: RgbColor,
    pub primary: RgbColor,
    pub secondary: RgbColor,
}

impl Default for ThemePreferences {
    fn default() -> Self {
        Self {
            background: RgbColor::new(0x2a, 0x28, 0x28),
            text: RgbColor::new(0xc3, 0xc5, 0xc0),
            primary: RgbColor::new(0xf0, 0x64, 0x49),
            secondary: RgbColor::new(0x70, 0xe8, 0xe3),
        }
    }
}

/// One opaque eight-bit RGB value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RgbColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl RgbColor {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    pub const fn packed(self) -> u32 {
        ((self.red as u32) << 16) | ((self.green as u32) << 8) | self.blue as u32
    }
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

/// Recently opened project files, most recent first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentProjects {
    paths: Vec<PathBuf>,
}

impl RecentProjects {
    /// Build an ordered list, deduplicating exact native paths and rejecting
    /// malformed or excessive saved data rather than truncating it.
    pub fn new(paths: impl IntoIterator<Item = PathBuf>) -> Result<Self, PreferencesError> {
        let mut recent = Vec::new();
        let mut seen = HashSet::new();
        let mut total_path_bytes = 0_usize;
        for path in paths {
            validate_saved_path(&path)?;
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
                if recent.len() == MAX_RECENT_PROJECTS {
                    return Err(PreferencesError::TooManyRecentProjects {
                        maximum: MAX_RECENT_PROJECTS,
                    });
                }
                recent.push(path);
            }
        }
        Ok(Self { paths: recent })
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Promote a successfully opened path to the front and evict the oldest
    /// item when the bounded menu is full.
    pub fn record_opened(&mut self, path: PathBuf) -> Result<bool, PreferencesError> {
        validate_saved_path(&path)?;
        if self.paths.first() == Some(&path) {
            return Ok(false);
        }
        self.paths.retain(|item| item != &path);
        self.paths.insert(0, path);
        self.paths.truncate(MAX_RECENT_PROJECTS);
        while total_native_path_bytes(&self.paths) > MAX_TOTAL_PATH_BYTES {
            self.paths.pop();
        }
        Ok(true)
    }
}

fn total_native_path_bytes(paths: &[PathBuf]) -> usize {
    paths.iter().map(|path| native_path_len(path)).sum()
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

/// Load saved folders. A missing file is the default; malformed or oversized
/// files return an error and are left untouched.
pub fn load(path: &Path) -> Result<BrowserPlaces, PreferencesError> {
    let Some(bytes) = read_document(path)? else {
        return Ok(BrowserPlaces::default());
    };
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

/// Load recent project paths. Missing settings are empty; malformed files are
/// returned as errors without modification.
pub fn load_recent_projects(path: &Path) -> Result<RecentProjects, PreferencesError> {
    let bytes = read_document(path)?;
    let Some(bytes) = bytes else {
        return Ok(RecentProjects::default());
    };
    let document: RecentProjectsDocument = serde_json::from_slice(&bytes)
        .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if document.version != DOCUMENT_VERSION {
        return Err(PreferencesError::InvalidDocument(format!(
            "unsupported version {}",
            document.version
        )));
    }
    let paths = document
        .paths
        .into_iter()
        .map(StoredPath::into_path)
        .collect::<Result<Vec<_>, _>>()?;
    RecentProjects::new(paths)
}

/// Load theme colors. Missing settings use the shipped palette; malformed or
/// future-version documents are returned as errors without modification.
pub fn load_theme_preferences(path: &Path) -> Result<ThemePreferences, PreferencesError> {
    let Some(bytes) = read_document(path)? else {
        return Ok(ThemePreferences::default());
    };
    let document: ThemePreferencesDocument = serde_json::from_slice(&bytes)
        .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if document.version != DOCUMENT_VERSION {
        return Err(PreferencesError::InvalidDocument(format!(
            "unsupported version {}",
            document.version
        )));
    }
    Ok(document.colors)
}

/// Atomically save theme colors without replacing malformed or future settings.
pub fn save_theme_preferences(
    path: &Path,
    colors: &ThemePreferences,
) -> Result<(), PreferencesError> {
    let _ = load_theme_preferences(path)?;
    let document = ThemePreferencesDocument {
        version: DOCUMENT_VERSION,
        colors: *colors,
    };
    let bytes = serde_json::to_vec(&document)
        .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(PreferencesError::DocumentTooLarge {
            maximum: MAX_DOCUMENT_BYTES,
        });
    }
    atomic_save(path, &bytes)
}

/// Atomically save the recent-project document without replacing malformed or
/// future-version settings.
pub fn save_recent_projects(path: &Path, recent: &RecentProjects) -> Result<(), PreferencesError> {
    let _ = load_recent_projects(path)?;
    let paths = recent
        .paths
        .iter()
        .map(|path| {
            validate_saved_path(path)?;
            StoredPath::from_path(path)
        })
        .collect::<Result<Vec<_>, PreferencesError>>()?;
    let document = RecentProjectsDocument {
        version: DOCUMENT_VERSION,
        paths,
    };
    let bytes = serde_json::to_vec(&document)
        .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(PreferencesError::DocumentTooLarge {
            maximum: MAX_DOCUMENT_BYTES,
        });
    }
    atomic_save(path, &bytes)
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
    atomic_save(path, &bytes)
}

fn read_document(path: &Path) -> Result<Option<Vec<u8>>, PreferencesError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
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

fn atomic_save(path: &Path, bytes: &[u8]) -> Result<(), PreferencesError> {
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

fn validate_saved_path(path: &Path) -> Result<(), PreferencesError> {
    if path.as_os_str().is_empty() {
        return Err(PreferencesError::EmptyPath);
    }
    validate_path_length(path)
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
#[serde(deny_unknown_fields)]
struct RecentProjectsDocument {
    version: u32,
    paths: Vec<StoredPath>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemePreferencesDocument {
    version: u32,
    colors: ThemePreferences,
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

    #[test]
    fn theme_preferences_default_and_round_trip() {
        let directory = TestDirectory::new();
        let path = directory.0.join("theme.json");
        let defaults = ThemePreferences::default();
        assert_eq!(
            load_theme_preferences(&path).expect("missing theme"),
            defaults
        );
        let custom = ThemePreferences {
            background: RgbColor::new(0x18, 0x20, 0x29),
            text: RgbColor::new(0xd9, 0xdf, 0xe6),
            primary: RgbColor::new(0xff, 0x74, 0x55),
            secondary: RgbColor::new(0x61, 0xdf, 0xd5),
        };
        save_theme_preferences(&path, &custom).expect("save theme");
        assert_eq!(load_theme_preferences(&path).expect("load theme"), custom);
        assert_eq!(custom.primary.packed(), 0xff7455);
    }

    #[test]
    fn malformed_and_future_theme_documents_are_preserved() {
        let directory = TestDirectory::new();
        let path = directory.0.join("theme.json");
        for bytes in [
            b"not json".as_slice(),
            br#"{"version":99,"colors":{"background":{"red":1,"green":2,"blue":3},"text":{"red":4,"green":5,"blue":6},"primary":{"red":7,"green":8,"blue":9},"secondary":{"red":10,"green":11,"blue":12}}}"#.as_slice(),
        ] {
            fs::write(&path, bytes).expect("write invalid theme fixture");
            assert!(matches!(
                load_theme_preferences(&path),
                Err(PreferencesError::InvalidDocument(_))
            ));
            assert!(matches!(
                save_theme_preferences(&path, &ThemePreferences::default()),
                Err(PreferencesError::InvalidDocument(_))
            ));
            assert_eq!(fs::read(&path).expect("read preserved theme"), bytes);
        }
    }

    #[test]
    fn theme_rgb_channels_are_bounded_by_the_serialized_type() {
        let directory = TestDirectory::new();
        let path = directory.0.join("theme.json");
        fs::write(
            &path,
            br#"{"version":1,"colors":{"background":{"red":256,"green":0,"blue":0},"text":{"red":0,"green":0,"blue":0},"primary":{"red":0,"green":0,"blue":0},"secondary":{"red":0,"green":0,"blue":0}}}"#,
        )
        .expect("write invalid channel fixture");
        assert!(matches!(
            load_theme_preferences(&path),
            Err(PreferencesError::InvalidDocument(_))
        ));
    }

    #[test]
    fn recent_projects_promote_exact_paths_deduplicate_and_evict_oldest() {
        let mut recent = RecentProjects::default();
        for index in 0..=MAX_RECENT_PROJECTS {
            assert!(
                recent
                    .record_opened(PathBuf::from(format!("/project-{index}.hex")))
                    .expect("record opened path")
            );
        }
        assert_eq!(recent.paths().len(), MAX_RECENT_PROJECTS);
        assert_eq!(recent.paths()[0], PathBuf::from("/project-10.hex"));
        assert_eq!(
            recent.paths()[MAX_RECENT_PROJECTS - 1],
            PathBuf::from("/project-1.hex")
        );
        assert!(
            recent
                .record_opened(PathBuf::from("/project-5.hex"))
                .expect("promote existing path")
        );
        assert_eq!(recent.paths()[0], PathBuf::from("/project-5.hex"));
        assert_eq!(recent.paths().len(), MAX_RECENT_PROJECTS);
        assert!(
            !recent
                .record_opened(PathBuf::from("/project-5.hex"))
                .expect("same most-recent path is unchanged")
        );
        assert_eq!(recent.paths()[0], PathBuf::from("/project-5.hex"));
    }

    #[test]
    fn recent_project_promotion_evicts_oldest_to_keep_combined_path_bytes_bounded() {
        let long_a = PathBuf::from(format!("/{}", "a".repeat(4_090)));
        let long_b = PathBuf::from(format!("/{}", "b".repeat(4_090)));
        let mut recent = RecentProjects::new([long_a.clone(), PathBuf::from("/short.hex")])
            .expect("initial recent projects");
        recent
            .record_opened(long_b.clone())
            .expect("promote valid long path");
        assert_eq!(
            recent.paths(),
            &[long_b.clone(), long_a.clone(), PathBuf::from("/short.hex")]
        );
        recent
            .record_opened(PathBuf::from("/new.hex"))
            .expect("promote short path");
        assert_eq!(
            recent.paths(),
            &[PathBuf::from("/new.hex"), long_b, long_a,]
        );
    }

    #[test]
    fn recent_projects_round_trip_paths_without_requiring_files_to_exist() {
        let directory = TestDirectory::new();
        let config = directory.0.join("recent-projects.json");
        let recent = RecentProjects::new([
            PathBuf::from("/offline/session.hex"),
            PathBuf::from("/other/session.hex"),
        ])
        .expect("recent projects");
        save_recent_projects(&config, &recent).expect("save recent projects");
        assert_eq!(load_recent_projects(&config).expect("load recents"), recent);
    }

    #[cfg(unix)]
    #[test]
    fn recent_projects_preserve_non_utf8_path_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let directory = TestDirectory::new();
        let config = directory.0.join("recent-projects.json");
        let path = PathBuf::from(OsString::from_vec(b"/tmp/session-\xff.hex".to_vec()));
        let recent = RecentProjects::new([path]).expect("native path");
        save_recent_projects(&config, &recent).expect("save native path");
        assert_eq!(
            load_recent_projects(&config).expect("load native path"),
            recent
        );
    }

    #[test]
    fn recent_projects_refuse_to_overwrite_malformed_document() {
        let directory = TestDirectory::new();
        let config = directory.0.join("recent-projects.json");
        let corrupt = b"not json";
        fs::write(&config, corrupt).expect("write malformed document");
        let mut recent = RecentProjects::default();
        recent
            .record_opened(PathBuf::from("/opened.hex"))
            .expect("record path");
        assert!(matches!(
            save_recent_projects(&config, &recent),
            Err(PreferencesError::InvalidDocument(_))
        ));
        assert_eq!(fs::read(&config).expect("read preserved document"), corrupt);
    }
}
