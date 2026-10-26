//! Independent application import policy, outside project history. Filesystem work
//! belongs to a control/background worker; malformed documents remain untouched.
use super::{DOCUMENT_VERSION, PreferencesError, atomic_save, read_document};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioImportPreferences {
    pub auto_warp_beats: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    preferences: AudioImportPreferences,
}

pub fn audio_import_preferences_path(home: &Path) -> PathBuf {
    home.join("Library/Application Support/hex/audio-import.json")
}

/// Missing preferences explicitly select unwarped imports.
pub fn load_audio_import_preferences(
    path: &Path,
) -> Result<AudioImportPreferences, PreferencesError> {
    let Some(bytes) = read_document(path)? else {
        return Ok(Default::default());
    };
    let document: Document = serde_json::from_slice(&bytes)
        .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if document.version != DOCUMENT_VERSION {
        return Err(PreferencesError::InvalidDocument(format!(
            "unsupported version {}",
            document.version
        )));
    }
    Ok(document.preferences)
}

/// Atomic publication shares the existing bounded preference-file machinery.
/// Refuse to overwrite an unsupported or malformed existing document.
pub fn save_audio_import_preferences(
    path: &Path,
    preferences: AudioImportPreferences,
) -> Result<(), PreferencesError> {
    let _ = load_audio_import_preferences(path)?;
    let bytes = serde_json::to_vec(&Document {
        version: DOCUMENT_VERSION,
        preferences,
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
            let root = std::env::temp_dir().join(format!(
                "hex-audio-import-prefs-{}-{}",
                std::process::id(),
                super::super::NEXT_TEMPORARY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_is_off_and_roundtrip_is_independent() {
        let dir = Directory::new();
        let path = dir.0.join("audio-import.json");
        assert_eq!(
            load_audio_import_preferences(&path).unwrap(),
            AudioImportPreferences {
                auto_warp_beats: false
            }
        );
        assert!(!path.exists());
        let unrelated = dir.0.join("theme.json");
        std::fs::write(&unrelated, b"preserved").unwrap();
        for enabled in [true, false] {
            let preferences = AudioImportPreferences {
                auto_warp_beats: enabled,
            };
            save_audio_import_preferences(&path, preferences).unwrap();
            assert_eq!(load_audio_import_preferences(&path).unwrap(), preferences);
        }
        assert_eq!(std::fs::read(unrelated).unwrap(), b"preserved");
    }

    #[test]
    fn malformed_future_unknown_and_write_failure_are_controlled() {
        let dir = Directory::new();
        let path = dir.0.join("audio-import.json");
        for bytes in [
            b"bad json".as_slice(),
            br#"{"version":99,"preferences":{"auto_warp_beats":true}}"#,
            br#"{"version":1,"preferences":{"auto_warp_beats":true,"unknown":true}}"#,
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert!(load_audio_import_preferences(&path).is_err());
            assert!(save_audio_import_preferences(&path, Default::default()).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        let parent_file = dir.0.join("not-a-directory");
        std::fs::write(&parent_file, b"keep").unwrap();
        assert!(
            save_audio_import_preferences(&parent_file.join("prefs.json"), Default::default())
                .is_err()
        );
        assert_eq!(std::fs::read(parent_file).unwrap(), b"keep");
    }
}
