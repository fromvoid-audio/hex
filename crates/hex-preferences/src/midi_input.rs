//! Independent, versioned preferences for the selected live MIDI input.
//!
//! Preferences are read and written on the control side through the shared
//! bounded document reader and atomic publisher. Missing files select no port
//! and channel zero. Malformed, unknown, or future documents remain untouched.

use super::{PreferencesError, atomic_save, read_document};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAX_PORT_NAME_BYTES: usize = 1024;
const MIDI_INPUT_DOCUMENT_VERSION: u32 = 1;

/// A native MIDI input port selection. The operating system's exact port name
/// is retained without normalization; `id` remains opaque to preferences.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiInputPort {
    pub id: u32,
    pub name: String,
}

/// Persisted live MIDI input selection. Channel is zero based (`0..=15`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiInputPreferences {
    pub port: Option<MidiInputPort>,
    pub channel: u8,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    preferences: MidiInputPreferences,
}

/// Stable app-support path for selected MIDI input preferences.
pub fn midi_input_preferences_path(home: &Path) -> PathBuf {
    home.join("Library/Application Support/hex/midi-input.json")
}

/// Missing preferences leave hardware input disabled and select channel zero.
pub fn load_midi_input_preferences(path: &Path) -> Result<MidiInputPreferences, PreferencesError> {
    let Some(bytes) = read_document(path)? else {
        return Ok(MidiInputPreferences::default());
    };
    let document: Document = serde_json::from_slice(&bytes)
        .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if document.version != MIDI_INPUT_DOCUMENT_VERSION {
        return Err(PreferencesError::InvalidDocument(format!(
            "unsupported version {}",
            document.version
        )));
    }
    validate(&document.preferences)?;
    Ok(document.preferences)
}

/// Atomically save preferences, refusing to overwrite malformed or unsupported
/// existing data so the user can recover it.
pub fn save_midi_input_preferences(
    path: &Path,
    preferences: &MidiInputPreferences,
) -> Result<(), PreferencesError> {
    validate(preferences)?;
    let _ = load_midi_input_preferences(path)?;
    let bytes = serde_json::to_vec(&Document {
        version: MIDI_INPUT_DOCUMENT_VERSION,
        preferences: preferences.clone(),
    })
    .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if bytes.len() as u64 > super::MAX_DOCUMENT_BYTES {
        return Err(PreferencesError::DocumentTooLarge {
            maximum: super::MAX_DOCUMENT_BYTES,
        });
    }
    atomic_save(path, &bytes)
}

fn validate(preferences: &MidiInputPreferences) -> Result<(), PreferencesError> {
    if preferences.channel > 15 {
        return Err(PreferencesError::InvalidDocument(
            "midi channel must be between 0 and 15".into(),
        ));
    }
    if preferences
        .port
        .as_ref()
        .is_some_and(|port| port.name.len() > MAX_PORT_NAME_BYTES)
    {
        return Err(PreferencesError::InvalidDocument(format!(
            "midi port name exceeds {MAX_PORT_NAME_BYTES} bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "hex-midi-input-prefs-{}-{}",
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
    fn missing_preferences_disable_input_and_default_to_channel_zero() {
        let dir = Directory::new();
        let path = dir.0.join("midi-input.json");
        assert_eq!(
            load_midi_input_preferences(&path).unwrap(),
            MidiInputPreferences {
                port: None,
                channel: 0,
            }
        );
        assert!(!path.exists());
    }

    #[test]
    fn round_trip_preserves_port_identity_name_case_and_channel() {
        let dir = Directory::new();
        let path = dir.0.join("midi-input.json");
        let preferences = MidiInputPreferences {
            port: Some(MidiInputPort {
                id: u32::MAX,
                name: "My MIDI Interface (Port A)".into(),
            }),
            channel: 15,
        };
        save_midi_input_preferences(&path, &preferences).unwrap();
        assert_eq!(load_midi_input_preferences(&path).unwrap(), preferences);
    }

    #[test]
    fn invalid_channel_and_oversized_port_name_are_rejected() {
        let dir = Directory::new();
        let path = dir.0.join("midi-input.json");
        for preferences in [
            MidiInputPreferences {
                port: None,
                channel: 16,
            },
            MidiInputPreferences {
                port: Some(MidiInputPort {
                    id: 1,
                    name: "x".repeat(MAX_PORT_NAME_BYTES + 1),
                }),
                channel: 0,
            },
            MidiInputPreferences {
                port: Some(MidiInputPort {
                    id: 2,
                    name: "é".repeat((MAX_PORT_NAME_BYTES / 2) + 1),
                }),
                channel: 0,
            },
        ] {
            assert!(save_midi_input_preferences(&path, &preferences).is_err());
            assert!(!path.exists());
        }
    }

    #[test]
    fn malformed_future_unknown_and_oversized_name_documents_are_preserved() {
        let dir = Directory::new();
        let path = dir.0.join("midi-input.json");
        let oversized = format!(
            r#"{{"version":1,"preferences":{{"port":{{"id":7,"name":"{}"}},"channel":0}}}}"#,
            "x".repeat(MAX_PORT_NAME_BYTES + 1)
        );
        let documents = [
            b"bad json".to_vec(),
            br#"{"version":99,"preferences":{"port":null,"channel":0}}"#.to_vec(),
            br#"{"version":1,"preferences":{"port":null,"channel":0,"unknown":true}}"#.to_vec(),
            oversized.into_bytes(),
        ];
        for bytes in documents {
            std::fs::write(&path, &bytes).unwrap();
            assert!(load_midi_input_preferences(&path).is_err());
            assert!(save_midi_input_preferences(&path, &MidiInputPreferences::default()).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn write_failure_preserves_existing_path_contents() {
        let dir = Directory::new();
        let path = dir.0.join("not-a-directory");
        std::fs::write(&path, b"preserve me").unwrap();
        assert!(
            save_midi_input_preferences(
                &path.join("midi-input.json"),
                &MidiInputPreferences::default()
            )
            .is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), b"preserve me");
    }
}
