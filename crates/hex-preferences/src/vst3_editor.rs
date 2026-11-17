//! Versioned keyboard policy for the app's native VST3 editor helpers.
use super::{DOCUMENT_VERSION, PreferencesError, read_document};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Vst3EditorPreferences {
    pub spacebar_controls_transport: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    preferences: Vst3EditorPreferences,
}

pub fn vst3_editor_preferences_path(home: &Path) -> PathBuf {
    home.join("Library/Application Support/hex/vst3-editor.json")
}

/// Missing files and omitted flags leave the plug-in's keyboard routing on.
pub fn load_vst3_editor_preferences(
    path: &Path,
) -> Result<Vst3EditorPreferences, PreferencesError> {
    let Some(bytes) = read_document(path)? else {
        return Ok(Vst3EditorPreferences::default());
    };
    decode(&bytes)
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

    #[test]
    fn transport_space_is_opt_in_and_requires_a_known_config() {
        for (document, enabled) in [
            (r#"{"version":1,"preferences":{}}"#, false),
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
}
