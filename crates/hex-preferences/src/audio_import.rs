//! Independent application import policy, outside project history. Filesystem work
//! belongs to a control/background worker; malformed documents remain untouched.
use crate::storage::{
    DOCUMENT_VERSION, PreferencesError, VersionedDocument, app_support_dir_from, load_versioned,
    save_versioned,
};
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

impl VersionedDocument for Document {
    fn version(&self) -> u32 {
        self.version
    }
}

pub fn audio_import_preferences_path(home: &Path) -> PathBuf {
    app_support_dir_from(home).join("audio-import.json")
}

/// Missing preferences explicitly select unwarped imports.
pub fn load_audio_import_preferences(
    path: &Path,
) -> Result<AudioImportPreferences, PreferencesError> {
    load_versioned::<Document, _>(path, AudioImportPreferences::default, |document| {
        Ok(document.preferences)
    })
}

/// Atomic publication shares the existing bounded preference-file machinery.
/// Refuse to overwrite an unsupported or malformed existing document.
pub fn save_audio_import_preferences(
    path: &Path,
    preferences: AudioImportPreferences,
) -> Result<(), PreferencesError> {
    save_versioned(
        path,
        |existing| load_audio_import_preferences(existing).map(|_| ()),
        &Document {
            version: DOCUMENT_VERSION,
            preferences,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::TestDirectory;

    #[test]
    fn missing_is_off_and_roundtrip_is_independent() {
        let dir = TestDirectory::new("hex-audio-import-prefs");
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
        let dir = TestDirectory::new("hex-audio-import-prefs");
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
