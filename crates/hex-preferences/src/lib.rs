//! # Purpose
//! Own bounded versioned application preferences independent of project documents and history.
//!
//! # Dependencies
//! Uses serde/serde_json and the standard library for recoverable settings publication.
//!
//! # Does not own
//! Does not mutate projects, decode media or run filesystem work on UI/audio callbacks. Preserve
//! original paths and malformed settings instead of silently replacing user data.

mod track_defaults;
pub use track_defaults::{
    TrackDefaults, load_track_defaults, save_track_defaults, track_defaults_path,
};

mod audio_import;
pub use audio_import::{
    AudioImportPreferences, audio_import_preferences_path, load_audio_import_preferences,
    save_audio_import_preferences,
};

mod midi_input;
pub use midi_input::{
    MidiInputPort, MidiInputPreferences, load_midi_input_preferences, midi_input_preferences_path,
    save_midi_input_preferences,
};

mod project_directory;
pub use project_directory::{load_project_directory, save_project_directory};

mod vst3_editor;
pub use vst3_editor::{
    Vst3EditorPreferences, load_vst3_editor_preferences, vst3_editor_preferences_path,
};

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
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
const MAX_PLACE_ICON_ID_BYTES: usize = 64;
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

/// Stable location for the last successfully used project folder.
pub fn project_directory_preferences_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join("hex")
        .join("project-directory.json")
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
            background: RgbColor::new(0x1b, 0x1b, 0x1b),
            text: RgbColor::new(0xd2, 0xd2, 0xd2),
            primary: RgbColor::new(0x3b, 0x95, 0xdf),
            secondary: RgbColor::new(0x75, 0xba, 0xff),
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

/// Stable identities independent of the user's current Home path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserBuiltin {
    Home,
    Music,
    Samples,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct BuiltinPreference {
    hidden: bool,
    icon: Option<String>,
}

/// Additional browser folders, in user-visible order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BrowserPlaces {
    additional: Vec<PathBuf>,
    icons: BTreeMap<PathBuf, String>,
    builtins: BTreeMap<BrowserBuiltin, BuiltinPreference>,
    hidden_places: HashSet<PathBuf>,
    pub vst3_scan_sources: Vst3ScanSources,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vst3ScanSources {
    pub system: bool,
    pub user: bool,
    pub custom_enabled: bool,
    pub custom_folder: Option<PathBuf>,
}

impl Default for Vst3ScanSources {
    fn default() -> Self {
        Self {
            system: true,
            user: true,
            custom_enabled: false,
            custom_folder: None,
        }
    }
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
        Ok(Self {
            additional,
            icons: BTreeMap::new(),
            builtins: BTreeMap::new(),
            hidden_places: HashSet::new(),
            vst3_scan_sources: Vst3ScanSources::default(),
        })
    }

    pub fn builtin_icon(&self, source: BrowserBuiltin) -> Option<&str> {
        self.builtins
            .get(&source)
            .and_then(|value| value.icon.as_deref())
    }

    pub fn builtin_hidden(&self, source: BrowserBuiltin) -> bool {
        self.builtins.get(&source).is_some_and(|value| value.hidden)
    }

    pub fn set_builtin_icon(
        &mut self,
        source: BrowserBuiltin,
        icon: Option<&str>,
    ) -> Result<(), PreferencesError> {
        if let Some(id) = icon {
            validate_icon_id(id)?;
        }
        self.builtins.entry(source).or_default().icon = icon.map(str::to_owned);
        Ok(())
    }

    pub fn set_builtin_hidden(&mut self, source: BrowserBuiltin, hidden: bool) {
        self.builtins.entry(source).or_default().hidden = hidden;
    }

    pub fn place_hidden(&self, path: &Path) -> bool {
        self.hidden_places.contains(path)
    }

    pub fn set_place_hidden(&mut self, path: &Path, hidden: bool) -> Result<(), PreferencesError> {
        if !self.additional.iter().any(|place| place == path) {
            return Err(PreferencesError::InvalidDocument(
                "hidden source has no matching place".into(),
            ));
        }
        if hidden {
            self.hidden_places.insert(path.to_owned());
        } else {
            self.hidden_places.remove(path);
        }
        Ok(())
    }

    pub fn additional(&self) -> &[PathBuf] {
        &self.additional
    }

    /// Point an existing place at a moved folder, keeping its position, icon and
    /// hidden state. Refuses an unknown source or a destination that is already a
    /// place so two entries never alias one folder.
    pub fn relocate(&mut self, old: &Path, new: PathBuf) -> Result<(), PreferencesError> {
        if new.as_os_str().is_empty() {
            return Err(PreferencesError::EmptyPath);
        }
        validate_path_length(&new)?;
        if self.additional.contains(&new) {
            return Err(PreferencesError::InvalidDocument(
                "relocated folder is already a place".into(),
            ));
        }
        let index = self
            .additional
            .iter()
            .position(|place| place == old)
            .ok_or_else(|| {
                PreferencesError::InvalidDocument("relocated source has no matching place".into())
            })?;
        let total = self
            .additional
            .iter()
            .enumerate()
            .filter(|(position, _)| *position != index)
            .try_fold(native_path_len(&new), |total, (_, path)| {
                total.checked_add(native_path_len(path))
            });
        if total.is_none_or(|total| total > MAX_TOTAL_PATH_BYTES) {
            return Err(PreferencesError::DocumentTooLarge {
                maximum: MAX_DOCUMENT_BYTES,
            });
        }
        if let Some(icon) = self.icons.remove(old) {
            self.icons.insert(new.clone(), icon);
        }
        if self.hidden_places.remove(old) {
            self.hidden_places.insert(new.clone());
        }
        self.additional[index] = new;
        Ok(())
    }

    /// A bounded opaque bundled-icon identifier, keyed by the original native path.
    /// Unknown identifiers survive a save; the UI owns their visible fallback.
    pub fn icon(&self, path: &Path) -> Option<&str> {
        self.icons.get(path).map(String::as_str)
    }

    pub fn icon_assignments(&self) -> impl Iterator<Item = (&Path, &str)> {
        self.icons
            .iter()
            .map(|(path, id)| (path.as_path(), id.as_str()))
    }

    pub fn set_icon(&mut self, path: &Path, id: Option<&str>) -> Result<(), PreferencesError> {
        if !self.additional.iter().any(|place| place == path) {
            return Err(PreferencesError::InvalidDocument(
                "icon has no matching place".into(),
            ));
        }
        if let Some(id) = id {
            validate_icon_id(id)?;
            self.icons.insert(path.to_owned(), id.to_owned());
        } else {
            self.icons.remove(path);
        }
        Ok(())
    }
}

fn validate_icon_id(id: &str) -> Result<(), PreferencesError> {
    if id.is_empty() || id.len() > MAX_PLACE_ICON_ID_BYTES || id.chars().any(char::is_control) {
        return Err(PreferencesError::InvalidDocument(
            "invalid place icon identifier".into(),
        ));
    }
    Ok(())
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
    let mut places = BrowserPlaces::new(paths)?;
    if let Some(scan) = document.vst3_scan_sources {
        let custom_folder = scan.custom_folder.map(StoredPath::into_path).transpose()?;
        if let Some(path) = custom_folder.as_deref() {
            validate_saved_path(path)?;
        }
        places.vst3_scan_sources = Vst3ScanSources {
            system: scan.system,
            user: scan.user,
            custom_enabled: scan.custom_enabled,
            custom_folder,
        };
    }
    if document.place_icons.len() > MAX_USER_PLACES {
        return Err(PreferencesError::InvalidDocument(
            "too many place icons".into(),
        ));
    }
    let mut seen = HashSet::new();
    for icon in document.place_icons {
        let path = icon.path.into_path()?;
        if !seen.insert(path.clone()) {
            return Err(PreferencesError::InvalidDocument(
                "duplicate place icon".into(),
            ));
        }
        places.set_icon(&path, Some(&icon.id))?;
    }
    if document.builtin_sources.len() > 3 || document.hidden_places.len() > MAX_USER_PLACES {
        return Err(PreferencesError::InvalidDocument(
            "too many source preferences".into(),
        ));
    }
    for value in document.builtin_sources {
        let source = value.source;
        if let Some(id) = &value.icon {
            validate_icon_id(id)?;
        }
        if places
            .builtins
            .insert(
                source,
                BuiltinPreference {
                    hidden: value.hidden,
                    icon: value.icon,
                },
            )
            .is_some()
        {
            return Err(PreferencesError::InvalidDocument(
                "duplicate builtin identity".into(),
            ));
        }
    }
    for stored in document.hidden_places {
        let path = stored.into_path()?;
        if places.place_hidden(&path) {
            return Err(PreferencesError::InvalidDocument(
                "duplicate hidden place".into(),
            ));
        }
        places.set_place_hidden(&path, true)?;
    }
    Ok(places)
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
        vst3_scan_sources: (places.vst3_scan_sources != Vst3ScanSources::default())
            .then(|| {
                let scan = &places.vst3_scan_sources;
                let custom_folder = scan
                    .custom_folder
                    .as_deref()
                    .map(|path| {
                        validate_saved_path(path)?;
                        StoredPath::from_path(path)
                    })
                    .transpose()?;
                Ok::<_, PreferencesError>(StoredVst3ScanSources {
                    system: scan.system,
                    user: scan.user,
                    custom_enabled: scan.custom_enabled,
                    custom_folder,
                })
            })
            .transpose()?,
        builtin_sources: places
            .builtins
            .iter()
            .map(|(source, value)| StoredBuiltinPreference {
                source: *source,
                hidden: value.hidden,
                icon: value.icon.clone(),
            })
            .collect(),
        hidden_places: places
            .additional
            .iter()
            .filter(|path| places.place_hidden(path))
            .map(|path| StoredPath::from_path(path))
            .collect::<Result<Vec<_>, _>>()?,
        place_icons: places
            .icons
            .iter()
            .map(|(path, id)| {
                Ok(StoredPlaceIcon {
                    path: StoredPath::from_path(path)?,
                    id: id.clone(),
                })
            })
            .collect::<Result<Vec<_>, PreferencesError>>()?,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    place_icons: Vec<StoredPlaceIcon>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    builtin_sources: Vec<StoredBuiltinPreference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hidden_places: Vec<StoredPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vst3_scan_sources: Option<StoredVst3ScanSources>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredVst3ScanSources {
    system: bool,
    user: bool,
    custom_enabled: bool,
    custom_folder: Option<StoredPath>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredBuiltinPreference {
    source: BrowserBuiltin,
    hidden: bool,
    icon: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPlaceIcon {
    path: StoredPath,
    id: String,
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
    fn vst3_scan_sources_default_for_older_preferences_and_round_trip() {
        let directory = TestDirectory::new();
        let config = directory.0.join("browser.json");
        fs::write(&config, br#"{"version":1,"additional_places":[]}"#).unwrap();
        let mut places = load(&config).unwrap();
        assert_eq!(places.vst3_scan_sources, Vst3ScanSources::default());
        places.vst3_scan_sources = Vst3ScanSources {
            system: false,
            user: true,
            custom_enabled: false,
            custom_folder: Some(PathBuf::from("/offline/plug-ins")),
        };
        save(&config, &places).unwrap();
        assert_eq!(load(&config).unwrap(), places);
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
    #[test]
    fn place_icons_round_trip_by_native_identity_allow_reuse_and_preserve_unknown() {
        let directory = TestDirectory::new();
        let config = directory.0.join("browser.json");
        let first = directory.0.join("one/same");
        let second = directory.0.join("two/same");
        let mut places = BrowserPlaces::new([second.clone(), first.clone()]).unwrap();
        assert_eq!(places.icon(&first), None);
        places.set_icon(&first, Some("samples")).unwrap();
        places.set_icon(&second, Some("samples")).unwrap();
        save(&config, &places).unwrap();
        assert_eq!(load(&config).unwrap(), places);
        places
            .set_icon(&second, Some("future-bundled-icon"))
            .unwrap();
        save(&config, &places).unwrap();
        let loaded = load(&config).unwrap();
        assert_eq!(loaded.additional(), &[second.clone(), first.clone()]);
        assert_eq!(loaded.icon(&first), Some("samples"));
        assert_eq!(loaded.icon(&second), Some("future-bundled-icon"));
        places.set_icon(&first, None).unwrap();
        save(&config, &places).unwrap();
        assert_eq!(load(&config).unwrap().icon(&first), None);
        assert!(
            places
                .set_icon(&directory.0.join("absent"), Some("folder"))
                .is_err()
        );
        assert!(
            places
                .set_icon(&first, Some(&"x".repeat(MAX_PLACE_ICON_ID_BYTES + 1)))
                .is_err()
        );
        assert!(places.set_icon(&first, Some("bad\nicon")).is_err());
    }

    #[test]
    fn invalid_icon_metadata_refuses_load_and_preserves_original_on_save() {
        let directory = TestDirectory::new();
        let config = directory.0.join("browser.json");
        let stored = serde_json::json!({"encoding":"utf8", "value":"/added"});
        let icon = serde_json::json!({"path":stored.clone(), "id":"samples"});
        for icons in [
            serde_json::json!([icon.clone(), icon]),
            serde_json::json!([{"path":{"encoding":"utf8", "value":"/orphan"}, "id":"folder"}]),
            serde_json::json!([{"path":stored.clone(), "id":""}]),
            serde_json::json!([{"path":stored.clone(), "id":"x".repeat(65)}]),
        ] {
            let bytes = serde_json::to_vec(&serde_json::json!({"version":1,
                "additional_places":[stored.clone()], "place_icons":icons}))
            .unwrap();
            fs::write(&config, &bytes).unwrap();
            assert!(load(&config).is_err());
            assert!(save(&config, &BrowserPlaces::default()).is_err());
            assert_eq!(fs::read(&config).unwrap(), bytes);
        }
        fs::write(
            &config,
            br#"{"version":1,"additional_places":[{"encoding":"utf8","value":"/added"}]}"#,
        )
        .unwrap();
        assert_eq!(load(&config).unwrap().icon(Path::new("/added")), None);
    }

    #[cfg(unix)]
    #[test]
    fn place_icon_keys_preserve_non_utf8_native_paths() {
        use std::os::unix::ffi::OsStringExt;
        let directory = TestDirectory::new();
        let config = directory.0.join("browser.json");
        let path = directory.0.join(OsString::from_vec(vec![b'f', 0xff]));
        let mut places = BrowserPlaces::new([path.clone()]).unwrap();
        places.set_icon(&path, Some("waveform")).unwrap();
        save(&config, &places).unwrap();
        assert_eq!(load(&config).unwrap(), places);
        assert_eq!(load(&config).unwrap().icon(&path), Some("waveform"));
    }
    #[test]
    fn builtin_sources_and_hidden_places_round_trip_without_path_or_icon_loss() {
        let directory = TestDirectory::new();
        let config = directory.0.join("browser.json");
        let first = directory.0.join("one/same");
        let second = directory.0.join("two/same");
        let mut places = BrowserPlaces::new([first.clone(), second.clone()]).unwrap();
        places.set_icon(&first, Some("samples")).unwrap();
        places.set_place_hidden(&first, true).unwrap();
        places
            .set_builtin_icon(BrowserBuiltin::Home, Some("future-bundled-icon"))
            .unwrap();
        places
            .set_builtin_icon(BrowserBuiltin::Music, Some("samples"))
            .unwrap();
        for source in [
            BrowserBuiltin::Home,
            BrowserBuiltin::Music,
            BrowserBuiltin::Samples,
        ] {
            places.set_builtin_hidden(source, true);
        }
        save(&config, &places).unwrap();
        assert_eq!(load(&config).unwrap(), places);
        assert_eq!(
            load(&config).unwrap().additional(),
            &[first.clone(), second.clone()]
        );
        assert_eq!(
            load(&config).unwrap().builtin_icon(BrowserBuiltin::Home),
            Some("future-bundled-icon")
        );
        places.set_builtin_icon(BrowserBuiltin::Home, None).unwrap();
        places.set_builtin_hidden(BrowserBuiltin::Music, false);
        places.set_place_hidden(&first, false).unwrap();
        save(&config, &places).unwrap();
        assert_eq!(load(&config).unwrap(), places);
        assert!(!places.place_hidden(&second));
        assert_eq!(places.icon(&first), Some("samples"));
        assert!(
            !first.exists() && !second.exists(),
            "preferences must not create source folders"
        );
    }

    #[test]
    fn relocating_a_place_keeps_order_icon_and_hidden_state_and_refuses_aliases() {
        let directory = TestDirectory::new();
        let config = directory.0.join("browser.json");
        let [first, moved, last, new] =
            ["first", "old/samples", "last", "new/samples"].map(|name| directory.0.join(name));
        let mut places = BrowserPlaces::new([first.clone(), moved.clone(), last.clone()]).unwrap();
        places.set_icon(&moved, Some("samples")).unwrap();
        places.set_place_hidden(&moved, true).unwrap();
        places.relocate(&moved, new.clone()).unwrap();
        assert_eq!(
            places.additional(),
            &[first.clone(), new.clone(), last.clone()]
        );
        assert_eq!(places.icon(&new), Some("samples"));
        assert_eq!(places.icon(&moved), None);
        assert!(places.place_hidden(&new) && !places.place_hidden(&moved));
        save(&config, &places).unwrap();
        assert_eq!(load(&config).unwrap(), places);

        let unchanged = places.clone();
        assert!(places.relocate(&new, last.clone()).is_err());
        assert!(places.relocate(&moved, directory.0.join("other")).is_err());
        assert!(places.relocate(&new, PathBuf::new()).is_err());
        assert!(
            places
                .relocate(&new, PathBuf::from("/".repeat(MAX_PATH_BYTES + 1)))
                .is_err()
        );
        assert_eq!(places, unchanged, "refused relocation leaves places intact");
        assert!(!new.exists(), "preferences never create or move folders");
    }

    #[test]
    fn builtin_source_invalid_identity_duplicate_and_hidden_path_errors_preserve_document() {
        let directory = TestDirectory::new();
        let config = directory.0.join("browser.json");
        for sources in [
            serde_json::json!([{"source":"home","hidden":false,"icon":null},{"source":"home","hidden":true,"icon":null}]),
            serde_json::json!([{"source":"unknown-source","hidden":false,"icon":null}]),
            serde_json::json!([{"source":"music","hidden":false,"icon":"bad\nicon"}]),
        ] {
            let bytes = serde_json::to_vec(
                &serde_json::json!({"version":1,"additional_places":[],"builtin_sources":sources}),
            )
            .unwrap();
            fs::write(&config, &bytes).unwrap();
            assert!(load(&config).is_err());
            assert!(save(&config, &BrowserPlaces::default()).is_err());
            assert_eq!(fs::read(&config).unwrap(), bytes);
        }
        let mut places = BrowserPlaces::default();
        assert!(
            places
                .set_place_hidden(&directory.0.join("absent"), true)
                .is_err()
        );
        assert!(
            places
                .set_builtin_icon(BrowserBuiltin::Home, Some(""))
                .is_err()
        );
        assert!(
            places
                .set_builtin_icon(BrowserBuiltin::Home, Some(&"x".repeat(65)))
                .is_err()
        );
    }
    #[cfg(unix)]
    #[test]
    fn hidden_places_preserve_original_native_bytes_and_reject_orphan_records() {
        use std::os::unix::ffi::OsStringExt;
        let directory = TestDirectory::new();
        let config = directory.0.join("browser.json");
        let native = PathBuf::from(OsString::from_vec(b"/offline/same-\xff".to_vec()));
        let mut places = BrowserPlaces::new([native.clone()]).unwrap();
        places.set_place_hidden(&native, true).unwrap();
        save(&config, &places).unwrap();
        assert_eq!(load(&config).unwrap(), places);
        let stored = serde_json::to_value(StoredPath::from_path(&native).unwrap()).unwrap();
        for additional in [serde_json::json!([]), serde_json::json!([stored.clone()])] {
            let hidden = if additional.as_array().unwrap().is_empty() {
                serde_json::json!([stored.clone()])
            } else {
                serde_json::json!([stored.clone(), stored.clone()])
            };
            let bytes = serde_json::to_vec(&serde_json::json!({"version":1,"additional_places":additional,"hidden_places":hidden})).unwrap();
            fs::write(&config, &bytes).unwrap();
            assert!(load(&config).is_err());
            assert!(save(&config, &places).is_err());
            assert_eq!(fs::read(&config).unwrap(), bytes);
        }
    }
}
