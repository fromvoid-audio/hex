//! Validated new-track gain policy, independent of project data and history.
//! The persisted linear amplitude matches the track control's 0..=2 contract;
//! zero represents negative infinity dB. Filesystem work stays off UI/audio callbacks.
use super::{DOCUMENT_VERSION, PreferencesError, atomic_save, read_document};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Default gain for newly created ordinary tracks, stored as linear amplitude.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackDefaults {
    gain: f32,
}

const fn default_gain() -> f32 {
    // 10^(-12/20), rounded to f32, as used by the normal track gain control.
    0.251_188_64
}

impl Default for TrackDefaults {
    fn default() -> Self {
        Self {
            gain: default_gain(),
        }
    }
}

impl TrackDefaults {
    /// Construct a gain accepted by the canonical track volume control.
    pub fn new(gain: f32) -> Result<Self, PreferencesError> {
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return Err(PreferencesError::InvalidDocument(
                "track default gain must be finite and within 0..2".into(),
            ));
        }
        Ok(Self { gain })
    }

    pub fn gain(self) -> f32 {
        self.gain
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPreferences {
    #[serde(default = "default_gain")]
    gain: f32,
}
impl Default for StoredPreferences {
    fn default() -> Self {
        Self {
            gain: default_gain(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    #[serde(default)]
    preferences: StoredPreferences,
}

pub fn track_defaults_path(home: &Path) -> PathBuf {
    home.join("Library/Application Support/hex/track-defaults.json")
}

/// Missing documents and missing gain fields use the −12 dB default.
pub fn load_track_defaults(path: &Path) -> Result<TrackDefaults, PreferencesError> {
    let Some(bytes) = read_document(path)? else {
        return Ok(TrackDefaults::default());
    };
    let document: Document = serde_json::from_slice(&bytes)
        .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if document.version != DOCUMENT_VERSION {
        return Err(PreferencesError::InvalidDocument(format!(
            "unsupported version {}",
            document.version
        )));
    }
    TrackDefaults::new(document.preferences.gain)
}

/// Refuse to overwrite an unsupported or malformed document; publish atomically.
pub fn save_track_defaults(
    path: &Path,
    preferences: TrackDefaults,
) -> Result<(), PreferencesError> {
    let _ = load_track_defaults(path)?;
    let preferences = TrackDefaults::new(preferences.gain)?;
    let bytes = serde_json::to_vec(&Document {
        version: DOCUMENT_VERSION,
        preferences: StoredPreferences {
            gain: preferences.gain,
        },
    })
    .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    atomic_save(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hex-track-defaults-{}-{}",
                std::process::id(),
                super::super::NEXT_TEMPORARY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_and_absent_gain_default_to_minus_twelve_db_and_roundtrip() {
        let dir = Directory::new();
        let path = dir.0.join("track-defaults.json");
        let default = load_track_defaults(&path).unwrap();
        assert!((20.0 * default.gain().log10() + 12.0).abs() < 0.000_01);
        assert!(!path.exists());
        for data in [
            br#"{"version":1}"#.as_slice(),
            br#"{"version":1,"preferences":{}}"#,
        ] {
            std::fs::write(&path, data).unwrap();
            assert_eq!(load_track_defaults(&path).unwrap(), default);
        }
        for gain in [0.0, 0.125, 1.0, 2.0] {
            let value = TrackDefaults::new(gain).unwrap();
            save_track_defaults(&path, value).unwrap();
            assert_eq!(load_track_defaults(&path).unwrap(), value);
        }
    }

    #[test]
    fn invalid_values_and_malformed_documents_are_refused_without_overwrite() {
        let dir = Directory::new();
        let path = dir.0.join("track-defaults.json");
        for gain in [f32::NAN, f32::INFINITY, -0.1, 2.01] {
            assert!(TrackDefaults::new(gain).is_err());
        }
        for data in [
            b"bad json".as_slice(),
            br#"{"version":99,"preferences":{"gain":1}}"#,
            br#"{"version":1,"preferences":{"gain":2.1}}"#,
            br#"{"version":1,"preferences":{"gain":1,"unknown":true}}"#,
        ] {
            std::fs::write(&path, data).unwrap();
            assert!(load_track_defaults(&path).is_err());
            assert!(save_track_defaults(&path, TrackDefaults::default()).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), data);
        }
        let parent = dir.0.join("file");
        std::fs::write(&parent, b"keep").unwrap();
        assert!(save_track_defaults(&parent.join("prefs.json"), TrackDefaults::default()).is_err());
        assert_eq!(std::fs::read(parent).unwrap(), b"keep");
    }
}
