//! Versioned browser Places, builtin source visibility/icons, and VST3 scan
//! source preferences.
use crate::storage::{
    DOCUMENT_VERSION, MAX_DOCUMENT_BYTES, MAX_TOTAL_PATH_BYTES, PreferencesError, StoredPath,
    VersionedDocument, app_support_dir_from, load_versioned, native_path_len, save_versioned,
    validate_saved_path,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

pub const MAX_USER_PLACES: usize = 14;
pub const MAX_BROWSER_COLLECTIONS: usize = 32;
pub const MAX_COLLECTION_NAME_BYTES: usize = 64;
pub const MIN_SIDEBAR_WIDTH: u32 = 23;
pub const MAX_SIDEBAR_WIDTH: u32 = 640;
const MAX_PLACE_ICON_ID_BYTES: usize = 64;
const VALID_COLLECTION_COLORS: [&str; 10] = [
    "gray", "red", "orange", "amber", "olive", "green", "cyan", "blue", "violet", "rose",
];

/// Stable app-support location shared by development and review bundles.
pub fn browser_places_path(home: &Path) -> PathBuf {
    app_support_dir_from(home).join("browser.json")
}

/// Stable identities independent of the user's current Home path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserBuiltin {
    Home,
    Music,
    Samples,
}

/// Configurable library sections in the browser sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserSidebarSection {
    Samples,
    Devices,
    Projects,
    All,
    Plugins,
}

impl BrowserSidebarSection {
    pub const ALL: [Self; 5] = [
        Self::Samples,
        Self::Devices,
        Self::Projects,
        Self::All,
        Self::Plugins,
    ];
}

/// Persisted quick-collection identity and color swatch name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserCollectionPreference {
    pub name: String,
    pub color: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct BuiltinPreference {
    hidden: bool,
    icon: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SidebarSectionPreference {
    hidden: bool,
    icon: Option<String>,
}

/// Additional browser folders, sidebar configuration, quick collections, and pane preferences.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BrowserPlaces {
    additional: Vec<PathBuf>,
    icons: BTreeMap<PathBuf, String>,
    builtins: BTreeMap<BrowserBuiltin, BuiltinPreference>,
    hidden_places: HashSet<PathBuf>,
    sidebar_sections: BTreeMap<BrowserSidebarSection, SidebarSectionPreference>,
    sidebar_order: Vec<BrowserSidebarSection>,
    collections: Option<Vec<BrowserCollectionPreference>>,
    sidebar_width: Option<u32>,
    tags_visible: Option<bool>,
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
    /// empty, NUL-bearing, or excessive input rather than silently truncating saved data.
    pub fn new(paths: impl IntoIterator<Item = PathBuf>) -> Result<Self, PreferencesError> {
        let mut additional = Vec::new();
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
            sidebar_sections: BTreeMap::new(),
            sidebar_order: Vec::new(),
            collections: None,
            sidebar_width: None,
            tags_visible: None,
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
        validate_saved_path(&new)?;
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

    pub fn sidebar_section_hidden(&self, section: BrowserSidebarSection) -> bool {
        if let Some(pref) = self.sidebar_sections.get(&section) {
            pref.hidden
        } else if section == BrowserSidebarSection::Samples {
            self.builtin_hidden(BrowserBuiltin::Samples)
        } else {
            false
        }
    }

    pub fn set_sidebar_section_hidden(
        &mut self,
        section: BrowserSidebarSection,
        hidden: bool,
    ) -> Result<(), PreferencesError> {
        if hidden {
            let visible_others = BrowserSidebarSection::ALL
                .into_iter()
                .filter(|other| *other != section && !self.sidebar_section_hidden(*other))
                .count();
            if visible_others == 0 {
                return Err(PreferencesError::InvalidDocument(
                    "at least one browser section must stay visible".into(),
                ));
            }
        }
        self.sidebar_sections.entry(section).or_default().hidden = hidden;
        if section == BrowserSidebarSection::Samples {
            self.set_builtin_hidden(BrowserBuiltin::Samples, hidden);
        }
        Ok(())
    }

    pub fn sidebar_section_icon(&self, section: BrowserSidebarSection) -> Option<&str> {
        self.sidebar_sections
            .get(&section)
            .and_then(|pref| pref.icon.as_deref())
    }

    pub fn set_sidebar_section_icon(
        &mut self,
        section: BrowserSidebarSection,
        icon: Option<&str>,
    ) -> Result<(), PreferencesError> {
        if let Some(id) = icon {
            validate_icon_id(id)?;
        }
        self.sidebar_sections.entry(section).or_default().icon = icon.map(str::to_owned);
        Ok(())
    }

    pub fn sidebar_order(&self) -> &[BrowserSidebarSection] {
        &self.sidebar_order
    }

    pub fn set_sidebar_order(
        &mut self,
        order: Vec<BrowserSidebarSection>,
    ) -> Result<(), PreferencesError> {
        validate_sidebar_order(&order)?;
        self.sidebar_order = order;
        Ok(())
    }

    pub fn collections(&self) -> Option<&[BrowserCollectionPreference]> {
        self.collections.as_deref()
    }

    pub fn set_collections(
        &mut self,
        collections: Option<Vec<BrowserCollectionPreference>>,
    ) -> Result<(), PreferencesError> {
        if let Some(items) = collections {
            let validated = validate_collections(items)?;
            self.collections = Some(validated);
        } else {
            self.collections = None;
        }
        Ok(())
    }

    pub fn sidebar_width(&self) -> Option<u32> {
        self.sidebar_width
    }

    pub fn set_sidebar_width(&mut self, width: Option<u32>) -> Result<(), PreferencesError> {
        if let Some(value) = width
            && !(MIN_SIDEBAR_WIDTH..=MAX_SIDEBAR_WIDTH).contains(&value)
        {
            return Err(PreferencesError::InvalidDocument(
                "invalid browser sidebar width".into(),
            ));
        }
        self.sidebar_width = width;
        Ok(())
    }

    pub fn tags_visible(&self) -> Option<bool> {
        self.tags_visible
    }

    pub fn set_tags_visible(&mut self, visible: Option<bool>) {
        self.tags_visible = visible;
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

fn validate_sidebar_order(order: &[BrowserSidebarSection]) -> Result<(), PreferencesError> {
    if order.is_empty() {
        return Ok(());
    }
    if order.len() != BrowserSidebarSection::ALL.len() {
        return Err(PreferencesError::InvalidDocument(
            "invalid browser sidebar section order".into(),
        ));
    }
    let mut seen = HashSet::new();
    for section in order {
        if !seen.insert(*section) {
            return Err(PreferencesError::InvalidDocument(
                "duplicate browser sidebar section in order".into(),
            ));
        }
    }
    Ok(())
}

fn validate_collections(
    collections: Vec<BrowserCollectionPreference>,
) -> Result<Vec<BrowserCollectionPreference>, PreferencesError> {
    if collections.len() > MAX_BROWSER_COLLECTIONS {
        return Err(PreferencesError::InvalidDocument(
            "too many browser collections".into(),
        ));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(collections.len());
    for item in collections {
        let name = item.name.trim().to_lowercase();
        if name.is_empty()
            || name.len() > MAX_COLLECTION_NAME_BYTES
            || name.chars().any(char::is_control)
        {
            return Err(PreferencesError::InvalidDocument(
                "invalid browser collection name".into(),
            ));
        }
        let color = item.color.trim().to_lowercase();
        if !VALID_COLLECTION_COLORS.contains(&color.as_str()) {
            return Err(PreferencesError::InvalidDocument(
                "invalid browser collection color".into(),
            ));
        }
        if !seen.insert(name.clone()) {
            return Err(PreferencesError::InvalidDocument(
                "duplicate browser collection name".into(),
            ));
        }
        out.push(BrowserCollectionPreference { name, color });
    }
    Ok(out)
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sidebar_sections: Vec<StoredSidebarSectionPreference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sidebar_order: Vec<BrowserSidebarSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    collections: Option<Vec<BrowserCollectionPreference>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tags_visible: Option<bool>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSidebarSectionPreference {
    section: BrowserSidebarSection,
    hidden: bool,
    icon: Option<String>,
}

impl VersionedDocument for Document {
    fn version(&self) -> u32 {
        self.version
    }
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

/// Load saved folders. A missing file is the default; malformed or oversized
/// files return an error and are left untouched.
pub fn load(path: &Path) -> Result<BrowserPlaces, PreferencesError> {
    load_versioned::<Document, _>(path, BrowserPlaces::default, |document| {
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
        if document.sidebar_sections.len() > BrowserSidebarSection::ALL.len() {
            return Err(PreferencesError::InvalidDocument(
                "too many browser sidebar sections".into(),
            ));
        }
        let mut seen_sections = HashSet::new();
        for entry in document.sidebar_sections {
            if !seen_sections.insert(entry.section) {
                return Err(PreferencesError::InvalidDocument(
                    "duplicate browser sidebar section".into(),
                ));
            }
            if let Some(id) = &entry.icon {
                validate_icon_id(id)?;
            }
            places.sidebar_sections.insert(
                entry.section,
                SidebarSectionPreference {
                    hidden: entry.hidden,
                    icon: entry.icon,
                },
            );
        }
        let visible_sections = BrowserSidebarSection::ALL
            .into_iter()
            .filter(|section| !places.sidebar_section_hidden(*section))
            .count();
        if visible_sections == 0 {
            return Err(PreferencesError::InvalidDocument(
                "at least one browser section must stay visible".into(),
            ));
        }
        places.set_sidebar_order(document.sidebar_order)?;
        places.set_collections(document.collections)?;
        places.set_sidebar_width(document.sidebar_width)?;
        places.set_tags_visible(document.tags_visible);
        Ok(places)
    })
}

/// Atomically publish the latest complete preferences document.
pub fn save(path: &Path, places: &BrowserPlaces) -> Result<(), PreferencesError> {
    if places.additional.len() > MAX_USER_PLACES {
        return Err(PreferencesError::TooManyPlaces {
            maximum: MAX_USER_PLACES,
        });
    }
    let mut total_path_bytes = 0_usize;
    for path in &places.additional {
        validate_saved_path(path)?;
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
    validate_sidebar_order(&places.sidebar_order)?;
    if let Some(width) = places.sidebar_width
        && !(MIN_SIDEBAR_WIDTH..=MAX_SIDEBAR_WIDTH).contains(&width)
    {
        return Err(PreferencesError::InvalidDocument(
            "invalid browser sidebar width".into(),
        ));
    }
    let visible_sections = BrowserSidebarSection::ALL
        .into_iter()
        .filter(|section| !places.sidebar_section_hidden(*section))
        .count();
    if visible_sections == 0 {
        return Err(PreferencesError::InvalidDocument(
            "at least one browser section must stay visible".into(),
        ));
    }
    let collections = places
        .collections
        .clone()
        .map(validate_collections)
        .transpose()?;
    let sidebar_sections = places
        .sidebar_sections
        .iter()
        .filter(|(_, pref)| pref.hidden || pref.icon.is_some())
        .map(|(section, pref)| {
            if let Some(id) = &pref.icon {
                validate_icon_id(id)?;
            }
            Ok(StoredSidebarSectionPreference {
                section: *section,
                hidden: pref.hidden,
                icon: pref.icon.clone(),
            })
        })
        .collect::<Result<Vec<_>, PreferencesError>>()?;
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
        sidebar_sections,
        sidebar_order: places.sidebar_order.clone(),
        collections,
        sidebar_width: places.sidebar_width,
        tags_visible: places.tags_visible,
    };
    save_versioned(path, |existing| load(existing).map(|_| ()), &document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{MAX_PATH_BYTES, TestDirectory};
    use std::fs;

    #[test]
    fn vst3_scan_sources_default_for_older_preferences_and_round_trip() {
        let directory = TestDirectory::new("hex-browser-places");
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
        let directory = TestDirectory::new("hex-browser-places");
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
    fn duplicate_places_keep_first_order_and_empty_or_nul_paths_are_rejected() {
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
        assert!(matches!(
            BrowserPlaces::new([PathBuf::from("/bad\0place")]),
            Err(PreferencesError::InvalidDocument(_))
        ));
    }

    #[test]
    fn malformed_document_is_reported_without_changing_its_bytes() {
        let directory = TestDirectory::new("hex-browser-places");
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
        let directory = TestDirectory::new("hex-browser-places");
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
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let directory = TestDirectory::new("hex-browser-places");
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
    fn place_icons_round_trip_by_native_identity_allow_reuse_and_preserve_unknown() {
        let directory = TestDirectory::new("hex-browser-places");
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
        let directory = TestDirectory::new("hex-browser-places");
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
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let directory = TestDirectory::new("hex-browser-places");
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
        let directory = TestDirectory::new("hex-browser-places");
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
        let directory = TestDirectory::new("hex-browser-places");
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
        assert!(places.relocate(&new, PathBuf::from("/bad\0reloc")).is_err());
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
        let directory = TestDirectory::new("hex-browser-places");
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
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let directory = TestDirectory::new("hex-browser-places");
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

    #[test]
    fn sidebar_sections_order_collections_and_pane_prefs_round_trip_and_guard_last_section() {
        let directory = TestDirectory::new("hex-browser-places");
        let config = directory.0.join("browser.json");
        let custom_place = directory.0.join("packs");
        let mut places = BrowserPlaces::new([custom_place.clone()]).unwrap();
        places.set_icon(&custom_place, Some("waveform")).unwrap();
        places
            .set_sidebar_section_hidden(BrowserSidebarSection::Projects, true)
            .unwrap();
        places
            .set_sidebar_section_icon(BrowserSidebarSection::Devices, Some("waveform"))
            .unwrap();
        places
            .set_sidebar_order(vec![
                BrowserSidebarSection::Samples,
                BrowserSidebarSection::All,
                BrowserSidebarSection::Devices,
                BrowserSidebarSection::Plugins,
                BrowserSidebarSection::Projects,
            ])
            .unwrap();
        places
            .set_collections(Some(vec![
                BrowserCollectionPreference {
                    name: "  Drums ".into(),
                    color: "Amber".into(),
                },
                BrowserCollectionPreference {
                    name: "synths".into(),
                    color: "cyan".into(),
                },
            ]))
            .unwrap();
        places.set_sidebar_width(Some(148)).unwrap();
        places.set_tags_visible(Some(false));
        save(&config, &places).unwrap();

        let loaded = load(&config).unwrap();
        assert_eq!(loaded, places);
        assert_eq!(loaded.additional(), std::slice::from_ref(&custom_place));
        assert_eq!(loaded.icon(&custom_place), Some("waveform"));
        assert!(loaded.sidebar_section_hidden(BrowserSidebarSection::Projects));
        assert!(!loaded.sidebar_section_hidden(BrowserSidebarSection::Samples));
        assert_eq!(
            loaded.sidebar_section_icon(BrowserSidebarSection::Devices),
            Some("waveform")
        );
        assert_eq!(
            loaded.collections(),
            Some(
                [
                    BrowserCollectionPreference {
                        name: "drums".into(),
                        color: "amber".into(),
                    },
                    BrowserCollectionPreference {
                        name: "synths".into(),
                        color: "cyan".into(),
                    },
                ]
                .as_slice()
            )
        );
        assert_eq!(loaded.sidebar_width(), Some(148));
        assert_eq!(loaded.tags_visible(), Some(false));

        // Hiding the last remaining visible section is refused.
        let mut guarded = BrowserPlaces::default();
        for section in [
            BrowserSidebarSection::Samples,
            BrowserSidebarSection::Devices,
            BrowserSidebarSection::Projects,
            BrowserSidebarSection::Plugins,
        ] {
            guarded.set_sidebar_section_hidden(section, true).unwrap();
        }
        assert!(
            guarded
                .set_sidebar_section_hidden(BrowserSidebarSection::All, true)
                .is_err()
        );
        assert!(!guarded.sidebar_section_hidden(BrowserSidebarSection::All));
    }

    #[test]
    fn malformed_sidebar_and_collection_documents_fail_load_and_preserve_bytes() {
        let directory = TestDirectory::new("hex-browser-places");
        let config = directory.0.join("browser.json");
        let all_hidden = serde_json::json!({
            "version": 1,
            "additional_places": [],
            "sidebar_sections": [
                {"section": "samples", "hidden": true, "icon": null},
                {"section": "devices", "hidden": true, "icon": null},
                {"section": "projects", "hidden": true, "icon": null},
                {"section": "all", "hidden": true, "icon": null},
                {"section": "plugins", "hidden": true, "icon": null}
            ]
        });
        let dup_collection = serde_json::json!({
            "version": 1,
            "additional_places": [],
            "collections": [
                {"name": "drums", "color": "red"},
                {"name": "Drums", "color": "cyan"}
            ]
        });
        let bad_color = serde_json::json!({
            "version": 1,
            "additional_places": [],
            "collections": [{"name": "drums", "color": "neon"}]
        });
        let bad_order = serde_json::json!({
            "version": 1,
            "additional_places": [],
            "sidebar_order": ["samples", "samples", "all", "devices", "plugins"]
        });
        let bad_width = serde_json::json!({
            "version": 1,
            "additional_places": [],
            "sidebar_width": 5
        });
        for doc in [all_hidden, dup_collection, bad_color, bad_order, bad_width] {
            let bytes = serde_json::to_vec(&doc).unwrap();
            fs::write(&config, &bytes).unwrap();
            assert!(load(&config).is_err());
            assert!(save(&config, &BrowserPlaces::default()).is_err());
            assert_eq!(fs::read(&config).unwrap(), bytes);
        }
    }
}
