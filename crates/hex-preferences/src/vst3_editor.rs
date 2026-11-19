//! Versioned Settings storage for the app's native VST3 editor keyboard policy.
use super::{DOCUMENT_VERSION, PreferencesError, atomic_save, read_document};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Vst3EditorPreferences {
    pub spacebar_controls_transport: bool,
}

impl Default for Vst3EditorPreferences {
    fn default() -> Self {
        Self {
            spacebar_controls_transport: true,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    preferences: Vst3EditorPreferences,
}

pub fn vst3_editor_preferences_path(home: &Path) -> PathBuf {
    home.join("Library/Application Support/hex/vst3-editor.json")
}

/// Missing files and omitted flags reserve plain Space for Hex transport.
pub fn load_vst3_editor_preferences(
    path: &Path,
) -> Result<Vst3EditorPreferences, PreferencesError> {
    let Some(bytes) = read_document(path)? else {
        return Ok(Vst3EditorPreferences::default());
    };
    decode(&bytes)
}

/// Save the explicit Settings choice without replacing malformed or future files.
pub fn save_vst3_editor_preferences(
    path: &Path,
    preferences: Vst3EditorPreferences,
) -> Result<(), PreferencesError> {
    let _ = load_vst3_editor_preferences(path)?;
    let bytes = serde_json::to_vec(&Document {
        version: DOCUMENT_VERSION,
        preferences,
    })
    .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    atomic_save(path, &bytes)
}

fn decode(bytes: &[u8]) -> Result<Vst3EditorPreferences, PreferencesError> {
    let document: Document = serde_json::from_slice(bytes)
        .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if document.version != DOCUMENT_VERSION {
        return Err(PreferencesError::InvalidDocument(format!(
            "unsupported version {}",
            document.version
        )));
    }
    Ok(document.preferences)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::atomic::Ordering};

    #[test]
    fn transport_space_defaults_on_and_requires_a_known_config() {
        assert!(Vst3EditorPreferences::default().spacebar_controls_transport);
        for (document, enabled) in [
            (r#"{"version":1,"preferences":{}}"#, true),
            (
                r#"{"version":1,"preferences":{"spacebar_controls_transport":false}}"#,
                false,
            ),
            (
                r#"{"version":1,"preferences":{"spacebar_controls_transport":true}}"#,
                true,
            ),
        ] {
            assert_eq!(
                decode(document.as_bytes())
                    .unwrap()
                    .spacebar_controls_transport,
                enabled,
            );
        }
        assert!(decode(br#"{"version":2,"preferences":{}}"#).is_err());
        assert!(
            decode(br#"{"version":1,"preferences":{"spacebar_controls_transport":1}}"#).is_err()
        );
    }

    #[test]
    fn settings_save_preserves_a_future_document() {
        let folder = std::env::temp_dir().join(format!(
            "hex-vst3-editor-prefs-{}-{}",
            std::process::id(),
            super::super::NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&folder).unwrap();
        let path = vst3_editor_preferences_path(&folder);
        let bytes = br#"{"version":2,"preferences":{"spacebar_controls_transport":false}}"#;
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        let refusal = save_vst3_editor_preferences(&path, Vst3EditorPreferences::default());
        let preserved = fs::read(&path);
        fs::remove_dir_all(folder).unwrap();
        assert!(matches!(refusal, Err(PreferencesError::InvalidDocument(_))));
        assert_eq!(preserved.unwrap(), bytes);
    }
}
