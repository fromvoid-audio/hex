//! # Purpose
//! Own bounded versioned application preferences independent of project documents and history.
//!
//! # Dependencies
//! Uses serde/serde_json and the standard library for recoverable settings publication.
//!
//! # Does not own
//! Does not mutate projects, decode media or run filesystem work on UI/audio callbacks. Preserve
//! original paths and malformed settings instead of silently replacing user data.

mod audio_device;
mod audio_import;
mod browser_places;
mod midi_input;
mod playback;
mod project_directory;
mod recent_projects;
mod storage;
mod theme;
mod track_defaults;
mod vst3_editor;

pub use audio_device::{
    AudioDevicePreferences, SUPPORTED_BUFFER_SIZES, SUPPORTED_SAMPLE_RATES,
    audio_device_preferences_path, load_audio_device_preferences, save_audio_device_preferences,
};
pub use audio_import::{
    AudioImportPreferences, audio_import_preferences_path, load_audio_import_preferences,
    save_audio_import_preferences,
};
pub use browser_places::{
    BrowserBuiltin, BrowserCollectionPreference, BrowserPlaces, BrowserSidebarSection,
    MAX_BROWSER_COLLECTIONS, MAX_COLLECTION_NAME_BYTES, MAX_SIDEBAR_WIDTH, MAX_USER_PLACES,
    MIN_SIDEBAR_WIDTH, Vst3ScanSources, browser_places_path, load, save,
};
pub use midi_input::{
    DEFAULT_MPE_MASTER_PITCH_BEND_SEMITONES, DEFAULT_MPE_MEMBER_PITCH_BEND_SEMITONES,
    MAX_MIDI_INPUT_PORTS, MAX_MPE_MASTER_PITCH_BEND_SEMITONES, MAX_MPE_MEMBER_PITCH_BEND_SEMITONES,
    MidiInputMpeZone, MidiInputMpeZoneKind, MidiInputPort, MidiInputPreferences,
    load_midi_input_preferences, midi_input_preferences_path, save_midi_input_preferences,
};
pub use playback::{
    PlaybackPreferences, load_playback_preferences, playback_preferences_path,
    save_playback_preferences,
};
pub use project_directory::{
    load_project_directory, project_directory_preferences_path, save_project_directory,
};
pub use recent_projects::{
    MAX_RECENT_PROJECTS, RecentProjects, load_recent_projects, recent_projects_path,
    save_recent_projects,
};
pub use storage::{PreferencesError, app_support_dir, app_support_dir_from};
pub use theme::{
    RgbColor, ThemePreferences, load_theme_preferences, save_theme_preferences,
    theme_preferences_path,
};
pub use track_defaults::{
    TrackDefaults, load_track_defaults, save_track_defaults, track_defaults_path,
};
pub use vst3_editor::{
    Vst3EditorPreferences, load_vst3_editor_preferences, save_vst3_editor_preferences,
    vst3_editor_preferences_path,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsString, path::Path};

    #[test]
    fn app_support_dir_centralizes_all_preference_paths_and_env_fallbacks() {
        let home = Path::new("/tmp/hex-home");
        let base = app_support_dir_from(home);
        #[cfg(target_os = "macos")]
        assert_eq!(base, home.join("Library/Application Support/hex"));
        #[cfg(all(not(target_os = "macos"), not(windows)))]
        assert_eq!(base, home.join(".local/share/hex"));
        #[cfg(windows)]
        assert_eq!(base, home.join("AppData/Roaming/hex"));

        assert_eq!(browser_places_path(home), base.join("browser.json"));
        assert_eq!(
            recent_projects_path(home),
            base.join("recent-projects.json")
        );
        assert_eq!(
            project_directory_preferences_path(home),
            base.join("project-directory.json")
        );
        assert_eq!(theme_preferences_path(home), base.join("theme.json"));
        assert_eq!(track_defaults_path(home), base.join("track-defaults.json"));
        assert_eq!(
            audio_device_preferences_path(home),
            base.join("audio-device.json")
        );
        assert_eq!(
            audio_import_preferences_path(home),
            base.join("audio-import.json")
        );
        assert_eq!(
            midi_input_preferences_path(home),
            base.join("midi-input.json")
        );
        assert_eq!(playback_preferences_path(home), base.join("playback.json"));
        assert_eq!(
            vst3_editor_preferences_path(home),
            base.join("vst3-editor.json")
        );

        assert!(storage::app_support_dir_with_env(None, None, None, None).is_err());
        assert!(
            storage::app_support_dir_with_env(Some(OsString::new()), None, None, None).is_err()
        );
        assert_eq!(
            storage::app_support_dir_with_env(
                Some(OsString::from("/tmp/hex-home")),
                None,
                None,
                None,
            )
            .unwrap(),
            base
        );
    }
}
