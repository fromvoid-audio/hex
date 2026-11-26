//! Independent application playback policy, outside project documents and history.
//! Filesystem work belongs to a control/background worker; rejected files remain untouched.
use crate::storage::{
    DOCUMENT_VERSION, PreferencesError, VersionedDocument, app_support_dir_from, load_versioned,
    save_versioned,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Application playback choices, including held-MIDI note chase and Live Mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlaybackPreferences {
    pub note_chase: bool,
    /// When enabled, playback-interrupting edits prompt for confirmation;
    /// when disabled (non-live mode), interrupting edits proceed immediately.
    pub live_mode: bool,
}

impl Default for PlaybackPreferences {
    fn default() -> Self {
        Self {
            note_chase: true,
            live_mode: true,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    preferences: PlaybackPreferences,
}

impl VersionedDocument for Document {
    fn version(&self) -> u32 {
        self.version
    }
}

/// Stable app-support location shared by development and review bundles.
pub fn playback_preferences_path(home: &Path) -> PathBuf {
    app_support_dir_from(home).join("playback.json")
}

/// Missing files and omitted flags enable note chase without writing defaults.
/// Malformed, unknown-field and unsupported-version documents return errors unchanged.
pub fn load_playback_preferences(path: &Path) -> Result<PlaybackPreferences, PreferencesError> {
    load_versioned::<Document, _>(path, PlaybackPreferences::default, |document| {
        Ok(document.preferences)
    })
}

/// Atomically publish an explicit choice using the shared bounded storage machinery.
/// Refuse to overwrite malformed, oversized or unsupported existing documents.
pub fn save_playback_preferences(
    path: &Path,
    preferences: PlaybackPreferences,
) -> Result<(), PreferencesError> {
    save_versioned(
        path,
        |existing| load_playback_preferences(existing).map(|_| ()),
        &Document {
            version: DOCUMENT_VERSION,
            preferences,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{MAX_DOCUMENT_BYTES, TestDirectory};
    use std::fs;

    #[test]
    fn missing_file_and_omitted_fields_default_on_without_writing() {
        let dir = TestDirectory::new("hex-playback-prefs");
        let path = playback_preferences_path(&dir.0);
        assert_eq!(path, app_support_dir_from(&dir.0).join("playback.json"));
        assert!(PlaybackPreferences::default().note_chase);
        assert!(PlaybackPreferences::default().live_mode);
        assert_eq!(
            load_playback_preferences(&path).unwrap(),
            PlaybackPreferences {
                note_chase: true,
                live_mode: true,
            }
        );
        assert!(!app_support_dir_from(&dir.0).exists());

        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for (bytes, expected) in [
            (
                br#"{"version":1,"preferences":{}}"#.as_slice(),
                PlaybackPreferences {
                    note_chase: true,
                    live_mode: true,
                },
            ),
            (
                br#"{"version":1,"preferences":{"note_chase":false}}"#.as_slice(),
                PlaybackPreferences {
                    note_chase: false,
                    live_mode: true,
                },
            ),
            (
                br#"{"version":1,"preferences":{"live_mode":false}}"#.as_slice(),
                PlaybackPreferences {
                    note_chase: true,
                    live_mode: false,
                },
            ),
        ] {
            fs::write(&path, bytes).unwrap();
            assert_eq!(load_playback_preferences(&path).unwrap(), expected);
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn on_and_off_roundtrip_without_changing_other_preferences() {
        let dir = TestDirectory::new("hex-playback-prefs");
        let path = playback_preferences_path(&dir.0);
        let unrelated = app_support_dir_from(&dir.0).join("theme.json");
        fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
        fs::write(&unrelated, b"preserved").unwrap();
        for note_chase in [false, true] {
            for live_mode in [false, true] {
                let preferences = PlaybackPreferences {
                    note_chase,
                    live_mode,
                };
                save_playback_preferences(&path, preferences).unwrap();
                assert_eq!(load_playback_preferences(&path).unwrap(), preferences);
                assert_eq!(fs::read(&unrelated).unwrap(), b"preserved");
            }
        }
    }

    #[test]
    fn malformed_future_unknown_and_wrong_type_documents_are_preserved() {
        let dir = TestDirectory::new("hex-playback-prefs");
        let path = dir.0.join("playback.json");
        for bytes in [
            b"bad json".as_slice(),
            br#"{"version":99,"preferences":{"note_chase":false}}"#,
            br#"{"version":1,"preferences":{"note_chase":true},"unknown":true}"#,
            br#"{"version":1,"preferences":{"note_chase":true,"unknown":true}}"#,
            br#"{"version":1,"preferences":{"note_chase":"off"}}"#,
            br#"{"version":1,"preferences":{"note_chase":null}}"#,
            br#"{"version":1,"preferences":{"live_mode":"off"}}"#,
            br#"{"version":1,"preferences":{"live_mode":null}}"#,
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(matches!(
                load_playback_preferences(&path),
                Err(PreferencesError::InvalidDocument(_))
            ));
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert!(matches!(
                save_playback_preferences(&path, PlaybackPreferences::default()),
                Err(PreferencesError::InvalidDocument(_))
            ));
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn oversized_documents_are_preserved() {
        let dir = TestDirectory::new("hex-playback-prefs");
        let path = dir.0.join("playback.json");
        let bytes = vec![b' '; MAX_DOCUMENT_BYTES as usize + 1];
        fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            load_playback_preferences(&path),
            Err(PreferencesError::DocumentTooLarge { .. })
        ));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(matches!(
            save_playback_preferences(&path, PlaybackPreferences::default()),
            Err(PreferencesError::DocumentTooLarge { .. })
        ));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn filesystem_failures_are_controlled_and_preserve_existing_files() {
        let dir = TestDirectory::new("hex-playback-prefs");
        let parent_file = dir.0.join("not-a-directory");
        fs::write(&parent_file, b"preserved").unwrap();
        let path = parent_file.join("playback.json");
        assert!(matches!(
            load_playback_preferences(&path),
            Err(PreferencesError::Io(_))
        ));
        let nested = parent_file.join("nested").join("playback.json");
        assert!(matches!(
            load_playback_preferences(&nested),
            Err(PreferencesError::Io(_))
        ));
        assert!(matches!(
            save_playback_preferences(&path, PlaybackPreferences::default()),
            Err(PreferencesError::Io(_))
        ));
        assert_eq!(fs::read(&parent_file).unwrap(), b"preserved");
    }
}
