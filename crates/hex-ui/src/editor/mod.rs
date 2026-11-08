//! Native canonical editor view and bounded UI intent coordination.
//!
//! Session owns the editable project and prepare-before-commit workers; this module owns
//! display/selection/focus and captured typed requests. Render cached summaries and visible
//! content, never decode or perform filesystem/audio setup in painting. Pending requests preserve
//! captured generation/revision/target identity. Canonical commit and native applied-source
//! acknowledgment are separate states; do not claim current playback from a submitted revision
//! alone. Native keyboard/pointer delivery uses the pinned excluded GPUI dependency boundary, with
//! no unsafe workspace bridge.
#[cfg(all(test, feature = "ui-test-support"))]
mod action_outcome_ui_tests;
mod arrangement;
mod arrangement_audio_panel;
mod arrangement_device_drop;
#[cfg(all(test, feature = "ui-test-support"))]
mod arrangement_device_drop_ui_test;
mod arrangement_focus;
#[cfg(all(test, feature = "ui-test-support"))]
mod arrangement_main_track_ui_test;
mod arrangement_sections;
mod arrangement_seek;
mod audio_admission;
mod audio_event_editor;
mod audio_unavailable;
mod automation_view;
mod bounce;
mod browser;
mod browser_input;
mod clip_action_scope;
mod clip_boundary;
#[cfg(all(test, feature = "ui-test-support"))]
mod clip_clipboard_ui_test;
mod clip_content_cache;
#[cfg(all(test, feature = "ui-test-support"))]
mod clip_drag_ui_test;
mod clip_gain;
#[cfg(all(test, feature = "ui-test-support"))]
mod clip_gain_ui_tests;
mod clip_name;
mod clip_range_fit;
mod clip_selection;
mod cpu_meter;
#[cfg(all(test, feature = "ui-test-support"))]
mod delete_ui_tests;
mod device_automation;
mod history_playback;
mod leftover_recovery;
#[cfg(all(test, feature = "ui-test-support"))]
mod navigation_metrics;
mod plugin_effect;
mod selection_play_start;
mod truncated_import;
#[cfg(all(test, feature = "ui-test-support"))]
mod worker_ui_fixture;
#[cfg(feature = "ui-test-support")]
pub(crate) use plugin_effect::fixture_catalog as plugin_effect_fixture_catalog;
#[cfg(feature = "ui-test-support")]
pub(crate) use plugin_effect::fixture_view as plugin_effect_fixture_view;
#[cfg(all(test, feature = "ui-test-support"))]
mod project_recovery_ui_test;
mod section_launch;
mod section_layout_preview;
mod track_header;
mod track_header_device_drop;
mod track_hierarchy_indent;
mod track_latency;
mod track_meters;
mod track_performance;
mod workspace_view;
#[cfg(all(test, feature = "ui-test-support"))]
mod workspace_view_ui_test;

mod group_copy;
mod group_selection;
mod midi_pending_drag;
mod pending_clip_move;
mod pending_edit;
mod pending_sample_drop;
#[cfg(test)]
mod test_environment;

mod activation;
#[cfg(all(test, feature = "ui-test-support"))]
mod activation_ui_test;
mod arranger_navigation;
mod audio_import_settings;
pub(crate) mod changelog;
mod clip_detail_resize;
#[cfg(all(test, feature = "ui-test-support"))]
mod clip_editor_loop_ui_tests;
mod clip_loop;
mod clip_paint;
#[cfg(all(test, feature = "ui-test-support"))]
mod clip_paint_ui_test;
mod clip_reverse;
mod clip_view;
#[cfg(all(test, feature = "ui-test-support"))]
mod clip_view_ui_test;
mod close;
mod computer_keyboard;
mod console_columns;
mod console_drop;
#[cfg(all(test, feature = "ui-test-support"))]
mod console_selection_ui_test;
mod console_track_page;
mod context_menu;
mod controller;
mod controller_input;
mod detail_context;
mod device_comparison;
mod device_gesture;
mod device_panel;
mod device_preset;
mod dialogs;
mod disk_overload;
mod display_error;
mod drag;
mod drag_autoscroll;
mod duplicate;
mod edit_cursor;
#[cfg(all(test, feature = "ui-test-support"))]
mod edit_cursor_ui_test;
mod export;
mod external_audio_drop;
#[cfg(all(test, feature = "ui-test-support"))]
mod external_audio_drop_ui_test;
mod fade_handles;
mod fades;
mod gain_lane;
mod gain_recording;
mod header;
mod input_quantization;
mod loop_rail;
#[cfg(all(test, feature = "ui-test-support"))]
mod loop_rail_ui_test;
mod loop_region;
#[cfg(all(test, feature = "ui-test-support"))]
mod loop_selection_ui_test;
mod menus;
mod midi;
mod midi_audition;
mod midi_canvas;
mod midi_created_selection;
mod midi_creation;
mod midi_cue;
mod midi_drag;
mod midi_draw;
mod midi_group_drag;
mod midi_marquee;
mod midi_note_copy;
mod midi_note_overwrite;
mod midi_origin_view;
mod midi_pan;
mod midi_pitch_zoom;
mod midi_recording;
mod midi_viewport_grid;
mod missing_media;
mod native_automation_recording;
mod track_defaults;
mod user_errors;
#[cfg(feature = "ui-test-support")]
pub(crate) use midi_recording::pressure_fixture::exercise as midi_pressure_fixture;
#[cfg(all(test, feature = "ui-test-support"))]
mod arrangement_chrome_ui_test;
mod arrangement_map;
mod console_mixer;
mod console_returns;
mod group_overview;
mod midi_loop_notes;
#[cfg(all(test, feature = "ui-test-support"))]
mod midi_loop_profile;
mod midi_ruler;
mod midi_scene;
mod midi_selection;
mod midi_source_controls;
mod midi_step;
mod midi_stroke;
mod midi_thumbnail;
mod midi_velocity;
mod midi_velocity_randomize;
mod midi_velocity_ui;
mod midi_wheel;
mod mixer;
mod musical_audio_controls;
mod musical_time;
mod musical_time_controls;
#[cfg(all(test, feature = "ui-test-support"))]
mod output_meter_ui_test;
mod overview_waveform;
mod play_intent;
mod poll_cadence;
mod project_directory;
mod project_rate_settings;
mod project_recovery;
mod project_spectrum;
mod recent_projects;
mod recovery;
mod recovery_state;
mod refresh;
mod render;
mod return_tracks;
mod routing_controls;
mod sample_recorder;
mod sampler_load;
mod sampler_parameter;
mod save;
mod scrub;
mod seek;
mod selection;
mod selection_fades;
mod session_audio_panel;
mod session_grid;
mod session_navigation;
#[cfg(all(test, feature = "ui-test-support"))]
mod session_navigation_ui_test;
mod session_route_view;
#[cfg(all(test, feature = "ui-test-support"))]
mod session_track_reorder_ui_test;
mod signed_frame_viewport;
#[cfg(all(test, feature = "ui-test-support"))]
mod snap_nudge_ui_test;
#[cfg(all(test, feature = "ui-test-support"))]
mod stream_read_failure_ui_test;
mod tempo_input;
#[cfg(all(test, feature = "ui-test-support"))]
mod tempo_input_ui_test;
mod theme_settings;
mod time_gesture;
mod time_move_preview;
mod time_selection;
pub(super) mod time_selection_profile;
mod time_selection_tool;
mod timeline_strip;
#[cfg(all(test, feature = "ui-test-support"))]
mod timeline_strip_ui_tests;
mod track_clipboard;
#[cfg(all(test, feature = "ui-test-support"))]
mod track_clipboard_ui_test;
#[cfg(all(test, feature = "ui-test-support"))]
mod track_compact_ui_test;
#[cfg(all(test, feature = "ui-test-support"))]
mod track_creation_ui_tests;
mod track_gain;
#[cfg(all(test, feature = "ui-test-support"))]
mod track_group_delete_ui_test;
#[cfg(all(test, feature = "ui-test-support"))]
mod track_keyboard_ui_test;
mod track_layout;
#[cfg(all(test, feature = "ui-test-support"))]
mod track_layout_ui_test;
mod track_name;
mod track_reorder;
#[cfg(all(test, feature = "ui-test-support"))]
mod track_reorder_ui_test;
#[cfg(all(test, feature = "ui-test-support"))]
mod track_selection_ui_test;
mod track_tags;
mod tracks;
mod transport;
mod trim;
mod tuning;
mod update_flow;
mod viewport;
mod waveform;
mod waveform_geometry;
mod waveform_loop;
mod waveform_stretch;
mod zoom_fit;

use crate::theme;
use audio_admission::AudioAdmissionStatus;
use duplicate::DuplicateIntent;
use gpui::{
    App, AppContext, Bounds, Context, FocusHandle, KeyBinding, Window, WindowBounds, WindowOptions,
    actions, px, size,
};
use hex_audio::prepared::Status;
use hex_engine::playback::{Playback, PlaybackAction, PlaybackCommand};
use hex_project::{ClipId, Command, SceneId, TrackId};
use hex_session::service::{Request, Service, ViewState};
use recent_projects::RecentProjectsState;
use recovery_state::RecoveryState;
use refresh::RedrawGate;
use std::{collections::VecDeque, rc::Rc, sync::Arc};
use viewport::Viewport;

pub(super) const UI_REQUEST_CAPACITY: usize = 32;
pub(super) const UI_EVENT_BUDGET: usize = 2;
const PLAYBACK_CANDIDATE_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum EditorMode {
    #[default]
    Arrangement,
    Session,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PendingPlaybackAction {
    pub action: PlaybackAction,
    pub generation: u64,
    pub revision: Option<u64>,
    pub(super) intent_token: Option<play_intent::IntentToken>,
}

struct PendingPlayback {
    project_revision: u64,
    project_generation: u64,
    state: Arc<ViewState>,
    playback: Playback,
    install_fresh_runtime: bool,
    fresh_runtime_fence: Option<hex_audio::prepared::FreshRuntimeFence>,
    prepared_revision: Option<Arc<hex_engine::PreparedRevision>>,
    seek_request_id: Option<u64>,
    device_preview_identity: Option<hex_session::service::DevicePreviewIdentity>,
    tuning_preview_identity: Option<hex_session::service::TuningPreviewIdentity>,
}

struct PendingResidentPublication {
    token: hex_audio::prepared::ResidentPublicationToken,
    project_revision: u64,
    project_generation: u64,
    state: Arc<ViewState>,
    install_fresh_runtime: bool,
    fresh_runtime_fence: Option<hex_audio::prepared::FreshRuntimeFence>,
    prepared_revision: Option<Arc<hex_engine::PreparedRevision>>,
    seek_request_id: Option<u64>,
    device_preview_identity: Option<hex_session::service::DevicePreviewIdentity>,
    tuning_preview_identity: Option<hex_session::service::TuningPreviewIdentity>,
}

enum PendingEditorAudioResult {
    Opened {
        project_generation: u64,
        restart_request: Option<u64>,
        audio: crate::EditorAudio,
    },
    Unavailable {
        project_generation: u64,
        restart_request: Option<u64>,
        error: hex_audio::OutputOpenError,
    },
    /// The worker kept the held output for a replacement not yet installed.
    Rebound {
        project_generation: u64,
        output_identity: u64,
    },
}

impl PendingEditorAudioResult {
    fn project_generation(&self) -> u64 {
        match self {
            Self::Opened {
                project_generation, ..
            }
            | Self::Unavailable {
                project_generation, ..
            }
            | Self::Rebound {
                project_generation, ..
            } => *project_generation,
        }
    }
    fn close(self) {
        if let Self::Opened { mut audio, .. } = self {
            audio.control.close();
        }
    }
}

struct PendingEditorPublicationObservation {
    context: hex_engine::AudioSourceContext,
    output_identity: u64,
    state: crate::EditorPublicationState,
}

#[derive(Clone)]
struct PlaybackCandidate {
    worker_requires_preroll: Option<bool>,
    observation: Option<Arc<hex_engine::PreparedPlaybackObservation>>,
    audio_revision: u64,
    project_generation: u64,
    state: Arc<ViewState>,
    prepared_revision: Option<Arc<hex_engine::PreparedRevision>>,
    seek_request_id: Option<u64>,
    device_preview_identity: Option<hex_session::service::DevicePreviewIdentity>,
    tuning_preview_identity: Option<hex_session::service::TuningPreviewIdentity>,
}

#[derive(Clone, Copy)]
pub(super) struct PendingSceneDuplicate {
    pub source_scene: SceneId,
    pub project_generation: u64,
    pub select_on_completion: bool,
}

actions!(
    hex_editor,
    [
        TogglePlay,
        ToggleTrackLatency,
        ToggleBrowser,
        ToggleTrackPerformance,
        SearchBrowser,
        CreateFadesOrSearch,
        ToggleSnapGrid,
        CoarserGrid,
        FinerGrid,
        ThemeSettingsEscape,
        BrowserPrevious,
        BrowserNext,
        BrowserActivate,
        BrowserPreviewSelected,
        BrowserRandomSample,
        BrowserCollapseFolder,
        BrowserEscape,
        ToggleSolo,
        AdditiveSolo,
        CancelDragOrStop,
        PreviousTrack,
        NextTrack,
        LaunchSessionSelection,
        Stop,
        ImportAudio,
        NewProject,
        NewStreamingProject,
        OpenProject,
        OpenStreamingProject,
        OpenRelinkedProject,
        RecoverUntitledProject,
        RecoverSavedProject,
        RecoverPreviousSave,
        RecoverKeptSession,
        OpenSettings,
        OpenTuning,
        CloseTuning,
        CommitTuningName,
        CancelTuningName,
        SaveProject,
        SaveAsProject,
        InstallUpdateOnQuit,
        SaveAndRestartForUpdate,
        CancelUpdate,
        ExportProject,
        Undo,
        Redo,
        MoveLeft,
        MoveRight,
        MoveSceneUp,
        MoveSceneDown,
        TrimLeft,
        TrimRight,
        DeleteClip,
        DeleteSectionMarker,
        GroupSelection,
        RenameClip,
        RenameTrack,
        SelectPreviousReturn,
        SelectNextReturn,
        CommitTrackName,
        CancelTrackName,
        CommitClipName,
        CancelClipName,
        CopyTimeSelection,
        CutTimeSelection,
        PasteTimeSelection,
        InsertTimeSelection,
        DuplicateClip,
        DuplicateAudioEvent,
        DuplicateAliasClip,
        MakeClipUnique,
        SplitClip,
        LoopSelection,
        BounceToNewTracks,
        BounceAndMuteSources,
        ToggleLoop,
        ZoomIn,
        ZoomOut,
        FitTimeline,
        ZoomToFit,
        ZoomBack,
        PanTimelineLeft,
        PanTimelineRight,
        AddTrack,
        AddMidiTrack,
        AddMidiClip,
        RemoveTrack,
        RemoveHeaderTracks,
        SelectAllMidiNotes,
        ToggleMidiDrawMode,
        MidiOctaveUp,
        MidiOctaveDown,
        Quit
    ]
);

#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = hex_editor, no_json)]
pub(super) struct OpenRecentProject {
    pub(super) path: std::path::PathBuf,
}

fn configure_editor_app(cx: &mut App) -> Result<(), String> {
    theme::load_fonts(cx)?;
    bind_editor_keys(cx);
    menus::install(cx, &[]);
    dialogs::install_project_actions(cx);
    cx.on_window_closed(|cx| {
        if cx.windows().is_empty() {
            close::quit_confirmed(cx);
        }
    })
    .detach();
    Ok(())
}

// Text drafts own printable keys; modified commands retain their finalization paths.
const EDITOR_PRINTABLE_KEY_CONTEXT: &str = "hex_editor && !hex_text_editing";

fn bind_editor_keys(cx: &mut App) {
    crate::tooltips::bind_keys(cx);
    changelog::bind_keys(cx);
    hex_widgets::bind_response_graph_keys(cx);
    device_panel::bind_operator_keys(cx);
    device_panel::bind_picker_keys(cx);
    device_panel::bind_keyboard_keys(cx);
    section_launch::bind_keys(cx);
    clip_name::bind_keys(cx);
    track_name::bind_keys(cx);
    console_returns::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("escape", ThemeSettingsEscape, Some("hex_theme_settings")),
        KeyBinding::new("s", ToggleSolo, Some(EDITOR_PRINTABLE_KEY_CONTEXT)),
        KeyBinding::new("shift-s", AdditiveSolo, Some(EDITOR_PRINTABLE_KEY_CONTEXT)),
        KeyBinding::new("up", PreviousTrack, Some("hex_editor")),
        KeyBinding::new("down", NextTrack, Some("hex_editor")),
        KeyBinding::new("enter", LaunchSessionSelection, Some("hex_editor")),
        escape_binding(),
        KeyBinding::new("cmd-i", ImportAudio, Some("hex_editor")),
        KeyBinding::new("cmd-t", AddTrack, Some(EDITOR_PRINTABLE_KEY_CONTEXT)),
        KeyBinding::new("cmd-alt-b", ToggleBrowser, None),
        KeyBinding::new("cmd-f", CreateFadesOrSearch, None),
        KeyBinding::new("up", BrowserPrevious, Some("hex_browser")),
        KeyBinding::new("down", BrowserNext, Some("hex_browser")),
        KeyBinding::new("enter", BrowserActivate, Some("hex_browser")),
        KeyBinding::new("right", BrowserPreviewSelected, Some("hex_browser")),
        KeyBinding::new("r", BrowserRandomSample, Some("hex_browser")),
        KeyBinding::new("left", BrowserCollapseFolder, Some("hex_browser")),
        KeyBinding::new("escape", BrowserEscape, Some("hex_browser")),
        KeyBinding::new("up", BrowserPrevious, Some("hex_browser_search")),
        KeyBinding::new("down", BrowserNext, Some("hex_browser_search")),
        KeyBinding::new("enter", BrowserActivate, Some("hex_browser_search")),
        KeyBinding::new("escape", BrowserEscape, Some("hex_browser_search")),
        KeyBinding::new("cmd-shift-t", AddMidiTrack, Some("hex_editor")),
        KeyBinding::new("cmd-shift-m", AddMidiClip, Some("hex_editor")),
        KeyBinding::new("cmd-o", OpenProject, Some("hex_editor")),
        KeyBinding::new("cmd-n", NewProject, Some("hex_editor")),
        KeyBinding::new("cmd-s", SaveProject, Some("hex_editor")),
        KeyBinding::new("cmd-shift-s", SaveAsProject, Some("hex_editor")),
        KeyBinding::new("cmd-shift-e", ExportProject, Some("hex_editor")),
        KeyBinding::new("cmd-alt-t", OpenTuning, None),
        KeyBinding::new("escape", CloseTuning, Some("hex_tuning")),
        KeyBinding::new("enter", CommitTuningName, Some("hex_tuning_name")),
        KeyBinding::new("escape", CancelTuningName, Some("hex_tuning_name")),
        KeyBinding::new("cmd-z", Undo, Some("hex_tuning")),
        KeyBinding::new("cmd-shift-z", Redo, Some("hex_tuning")),
        KeyBinding::new("cmd-z", Undo, Some("hex_editor")),
        KeyBinding::new("cmd-z", Undo, Some("hex_gain_numeric_editor")),
        KeyBinding::new("cmd-shift-z", Redo, Some("hex_editor")),
        KeyBinding::new("cmd-shift-z", Redo, Some("hex_gain_numeric_editor")),
        KeyBinding::new("left", MoveLeft, Some("hex_editor")),
        KeyBinding::new("right", MoveRight, Some("hex_editor")),
        KeyBinding::new("shift-left", TrimLeft, Some("hex_editor")),
        KeyBinding::new("shift-right", TrimRight, Some("hex_editor")),
        KeyBinding::new("backspace", DeleteClip, Some("hex_editor")),
        KeyBinding::new("delete", DeleteClip, Some("hex_editor")),
        KeyBinding::new(
            "x",
            DeleteClip,
            Some("hex_editor && !hex_text_editing && !hex_text_input && !hex_device_selection && !hex_response_graph_remove && !hex_track_header"),
        ),
        KeyBinding::new("cmd-backspace", DeleteSectionMarker, Some("hex_editor")),
        KeyBinding::new("cmd-delete", DeleteSectionMarker, Some("hex_editor")),
        KeyBinding::new(
            "g",
            GroupSelection,
            Some("hex_editor && !hex_text_editing && !hex_midi && !hex_device_selection"),
        ),
        KeyBinding::new("cmd-c", CopyTimeSelection, Some("hex_editor")),
        KeyBinding::new("cmd-shift-c", CopyTimeSelection, Some("hex_editor")),
        KeyBinding::new("cmd-x", CutTimeSelection, Some("hex_editor")),
        KeyBinding::new("cmd-v", PasteTimeSelection, Some("hex_editor")),
        KeyBinding::new("cmd-shift-v", InsertTimeSelection, Some("hex_editor")),
        KeyBinding::new("cmd-d", DuplicateAliasClip, Some("hex_editor")),
        KeyBinding::new("cmd-d", DuplicateClip, Some("hex_audio_events")),
        KeyBinding::new("d", DuplicateAudioEvent, Some("hex_audio_events && !hex_text_editing")),
        KeyBinding::new("d", DuplicateClip, Some("hex_editor && !hex_text_editing && !hex_midi && !hex_audio_events && !hex_device_selection")),
        KeyBinding::new("i", MakeClipUnique, Some("hex_editor && !hex_text_editing && !hex_midi && !hex_audio_events && !hex_device_selection")),
        KeyBinding::new("cmd-alt-up", MoveSceneUp, Some("hex_editor")),
        KeyBinding::new("cmd-alt-down", MoveSceneDown, Some("hex_editor")),
        KeyBinding::new("e", SplitClip, Some(EDITOR_PRINTABLE_KEY_CONTEXT)),
        KeyBinding::new("cmd-l", ToggleLoop, Some("hex_editor && !hex_text_editing")),
        // Live's `b` toggles draw mode; hex keeps that inside the midi editor
        // (`hex_midi`). Arrangement has no draw mode, so `b` bounces there.
        KeyBinding::new(
            "b",
            BounceToNewTracks,
            Some("hex_editor && !hex_text_editing && !hex_midi && !hex_audio_events && !hex_device_selection"),
        ),
        KeyBinding::new(
            "cmd-b",
            BounceAndMuteSources,
            Some("hex_editor && !hex_text_editing && !hex_midi && !hex_audio_events"),
        ),
        KeyBinding::new("l", LoopSelection, Some(EDITOR_PRINTABLE_KEY_CONTEXT)),
        KeyBinding::new("w", LoopSelection, Some(EDITOR_PRINTABLE_KEY_CONTEXT)),
        KeyBinding::new("cmd-=", ZoomIn, Some("hex_editor")),
        KeyBinding::new(
            "+",
            CoarserGrid,
            Some("hex_editor && !hex_text_editing && !hex_midi"),
        ),
        KeyBinding::new(
            "-",
            FinerGrid,
            Some("hex_editor && !hex_text_editing && !hex_midi"),
        ),
        KeyBinding::new("cmd-+", ZoomIn, Some("hex_editor")),
        KeyBinding::new("cmd--", ZoomOut, Some("hex_editor")),
        KeyBinding::new("cmd-0", FitTimeline, Some("hex_editor")),
        KeyBinding::new("z", ZoomToFit, Some("hex_editor && !hex_text_editing")),
        KeyBinding::new("v", ZoomBack, Some("hex_editor && !hex_text_editing && !hex_text_input")),
        KeyBinding::new("alt-left", PanTimelineLeft, Some("hex_editor")),
        KeyBinding::new("alt-right", PanTimelineRight, Some("hex_editor")),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-a", SelectAllMidiNotes, Some("hex_midi")),
        KeyBinding::new("cmd-a", SelectAllMidiNotes, Some("hex_audio_events")),
        KeyBinding::new("cmd-4", ToggleSnapGrid, Some("hex_editor")),
        KeyBinding::new("b", ToggleMidiDrawMode, Some("hex_midi")),
        KeyBinding::new("shift-up", MidiOctaveUp, Some("hex_midi")),
        KeyBinding::new("shift-down", MidiOctaveDown, Some("hex_midi")),
    ]);
    hex_widgets::bind_numeric_control_keys(cx);
    hex_widgets::bind_text_input_keys(cx);
    // Background clip shortcuts have no target while the foreground gain lanes
    // own Arrangement focus. Child text, MIDI and event contexts keep their keys.
    cx.bind_keys(
        [
            "cmd-c",
            "cmd-shift-c",
            "cmd-x",
            "cmd-v",
            "cmd-shift-v",
            "left",
            "right",
            "shift-left",
            "shift-right",
            "cmd-shift-m",
            "cmd-r",
            "cmd-d",
            "d",
            "i",
            "e",
            "b",
            "cmd-b",
            "r",
            "g",
        ]
        .map(|key| KeyBinding::new(key, gpui::NoAction, Some("hex_arrangement_automation"))),
    );
    // Header commands retain track clipboard/rename semantics; these general
    // Arrangement commands would otherwise edit the retained background clips.
    cx.bind_keys(
        [
            "left",
            "right",
            "shift-left",
            "shift-right",
            "cmd-shift-m",
            "b",
            "cmd-b",
            "i",
        ]
        .map(|key| KeyBinding::new(key, gpui::NoAction, Some("hex_automation_track_header"))),
    );
}

fn escape_binding() -> KeyBinding {
    KeyBinding::new("escape", CancelDragOrStop, Some("hex_editor"))
}

fn editor_window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1240.0), px(760.0)), cx);
    WindowOptions {
        titlebar: Some(gpui::TitlebarOptions {
            title: Some("hex".into()),
            appears_transparent: true,
            hide_native_window_controls: cfg!(target_os = "macos"),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(880.0), px(540.0))),
        ..Default::default()
    }
}

fn install_close_guard(window: &mut Window, cx: &mut App, view: gpui::WeakEntity<EditorView>) {
    window.on_window_should_close(cx, move |window, cx| {
        view.update(cx, |view, cx| view.request_window_close(window, cx))
            .unwrap_or(true)
    });
}

pub(super) fn open_backend(cx: &mut App, backend: crate::EditorBackend) -> Result<(), String> {
    configure_editor_app(cx)?;
    close::install_quit_gate(cx);
    cx.open_window(editor_window_options(cx), move |window, cx| {
        let view = cx.new(|cx| EditorView::new_with_backend(window, cx, backend));
        install_close_guard(window, cx, view.downgrade());
        view
    })
    .map_err(|error| format!("could not open hex: {error}"))?;
    Ok(())
}

#[cfg(feature = "ui-test-support")]
pub(crate) fn sampler_load_fixture(
    cx: &mut gpui::TestAppContext,
    backend: crate::EditorBackend,
    root: std::path::PathBuf,
    directory: std::path::PathBuf,
) -> Result<gpui::AnyWindowHandle, String> {
    if !root.is_absolute()
        || !root.starts_with(std::env::temp_dir())
        || !directory.starts_with(&root)
    {
        return Err("sampler fixture requires an owned temporary root".into());
    }
    let home = root.join("home");
    std::fs::create_dir_all(home.join("Music")).map_err(|e| e.to_string())?;
    // Catalog publication refuses linked ancestors, including macOS's /var
    // temporary alias. Keep library paths as supplied, but use the physical
    // owned root for preference/catalog persistence.
    let preferences = root
        .canonicalize()
        .map_err(|error| error.to_string())?
        .join("browser.json");
    let places =
        hex_preferences::BrowserPlaces::new(vec![directory.clone()]).map_err(|e| e.to_string())?;
    hex_preferences::save(&preferences, &places).map_err(|e| e.to_string())?;
    cx.update(move |cx| {
        // GPUI's headless TestPlatform uses NoopTextSystem, which cannot
        // register fonts. This fixture exercises input/render contracts only;
        // production startup still requires verified bundled fonts.
        bind_editor_keys(cx);
        menus::install(cx, &[]);
        dialogs::install_project_actions(cx);
        let window = cx
            .open_window(editor_window_options(cx), move |window, cx| {
                cx.new(|cx| {
                    let mut view = EditorView::new_with_browser_paths(
                        window,
                        cx,
                        backend.service,
                        Err("preparing audio output".into()),
                        (
                            preferences,
                            root.join("recent.json"),
                            root.join("theme.json"),
                            root.join("project-directory.json"),
                        ),
                        Some(sampler_load::FixturePaths { home, directory }),
                        #[cfg(test)]
                        test_environment::Inputs::default(),
                    );
                    view.audio_events = backend.audio_events;
                    view.source_reservation = Some(backend.source_reservation);
                    view
                })
            })
            .map_err(|e| e.to_string())?;
        Ok(window.into())
    })
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EnqueuedNativeDeviceDrop {
    pub(super) project_generation: u64,
    pub(super) revision: u64,
    pub(super) source: hex_session::service::NativeDeviceDropSource,
    pub(super) track: TrackId,
    pub(super) before: Option<hex_project::DeviceId>,
    pub(super) replace: Option<hex_project::DeviceId>,
}

pub(super) struct EditorView {
    service: Service,
    solo_observation: Option<hex_session::service::SoloViewObservation>,
    transport_trace: Option<crate::diagnostics::TransportTrace>,
    audio: Option<crate::EditorAudio>,
    retired_action_outcomes: Option<hex_audio::prepared::ActionOutcomeReader>,
    retired_action_message: Option<String>,
    audio_events: std::sync::mpsc::Receiver<crate::EditorAudioEvent>,
    source_reservation: Option<hex_session::SourcePlanReservation>,
    restart_request_id: u64,
    pending_restart_request: Option<(u64, u64)>,
    pending_recovery_generation: Option<u64>,
    /// Output whose default route changed and still awaits its recovery.
    route_follow: Option<u64>,
    /// Vst3 devices the current output reports as degraded.
    degraded_plugins: crate::editor_backend::DegradedDevices,
    pending_audio_result: Option<PendingEditorAudioResult>,
    pending_publication_observation: Option<PendingEditorPublicationObservation>,
    document_audio_mode: hex_session::DocumentAudioMode,
    recovery: RecoveryState,
    stopped_playback_restore: Option<audio_admission::StoppedPlaybackRestore>,
    latency_rebuild: Option<audio_admission::LatencyRebuild>,
    leftover_recovery: leftover_recovery::State,
    truncated_import: truncated_import::State,
    // Streamed documents replace their sampler output stopped; this frame
    // resumes playback once that edit settles (Pulse #3784).
    sampler_edit_resume: Option<u64>,
    // Play requested while the stopped restore renews effects (for example
    // vst3); it starts once that restore settles (Pulse #3784).
    deferred_play: Option<(PlaybackAction, Option<u64>, u64)>,
    offline_cursor: u64,
    pub(in crate::editor) state: Option<Arc<ViewState>>,
    selected_track: Option<TrackId>,
    track_selection: selection::TrackSelection,
    gain_lane: gain_lane::GainLane,
    automation_view: automation_view::AutomationView,
    audio_clip_controls: arrangement_audio_panel::controls::Controls,
    sample_bpm_control: Option<musical_audio_controls::sample_bpm::Binding>,
    project_tempo_control: Option<tempo_input::Binding>,
    track_gain_controls: track_gain::Controls,
    gain_recorder: gain_recording::Recorder,
    native_automation_recorder: native_automation_recording::Recorder,
    midi_recorder: midi_recording::Recorder,
    routing_open: bool,
    routing_page: usize,
    pub(in crate::editor) selected_clip: Option<ClipId>,
    clip_selection: clip_selection::ClipSelection,
    clip_name_edit: Option<clip_name::NameEdit>,
    sections: arrangement_sections::Sections,
    section_autoplay: bool,
    track_name: track_name::TrackNameState,
    track_tags: track_tags::State,
    pub(in crate::editor) mode: EditorMode,
    pub(in crate::editor) selected_scene: Option<SceneId>,
    pub(in crate::editor) selected_cell: Option<(TrackId, SceneId)>,
    pub(in crate::editor) session_copy_source: Option<ClipId>,
    pub(in crate::editor) session_track_start: usize,
    console_returns: console_returns::State,
    pub(in crate::editor) session_visible_columns: usize,
    pub(in crate::editor) session_quantization: hex_engine::LaunchQuantization,
    pub(in crate::editor) midi_selection: midi_selection::MidiSelection,
    pub(in crate::editor) midi_note_gesture: Option<midi_drag::NoteGesture>,
    pub(in crate::editor) midi_group_gesture: Option<midi_group_drag::GroupGesture>,
    pending_midi_drag: Option<midi_pending_drag::Pending>,
    midi_origin_history: midi_origin_view::State,
    pub(in crate::editor) midi_creation: Option<midi_creation::Creation>,
    pub(in crate::editor) midi_stroke: Option<midi_stroke::MidiStroke>,
    pub(in crate::editor) midi_draw_mode: bool,
    pub(in crate::editor) midi_draw_pitch_locked: bool,
    /// Source-relative insertion beat; reset with the existing piano_view_clip owner.
    /// Pure view state: never transport position or a canonical project edit.
    pub(in crate::editor) midi_marker: Option<u64>,
    midi_step: midi_step::StepEntry,
    midi_velocity: midi_velocity_ui::State,
    midi_velocity_randomize: midi_velocity_randomize::State,
    midi_pan: midi_pan::State,
    midi_ruler: midi_ruler::State,
    clip_loop: clip_loop::State,
    midi_time_selection: midi_ruler::TimeSelection,
    midi_audition: midi_audition::MidiAudition,
    pending_created_note_selection: Option<midi_created_selection::Pending>,
    pub(in crate::editor) midi_marquee: Option<midi_selection::Marquee>,
    pub(in crate::editor) midi_scene_cache: midi_scene::MidiSceneCache,
    pub(in crate::editor) midi_hit_cycle: Option<midi_scene::HitCycle>,
    // Only suppresses redundant pointer invalidations; paint resolves current geometry.
    pub(in crate::editor) midi_hover: Option<midi_scene::NoteHover>,
    piano_view_clip: Option<(u64, ClipId)>,
    piano_zoom_wheel_accum: f32,
    piano_pitch_wheel_accum: f32,
    poll_cadence: poll_cadence::PollCadence,
    piano_pitch_scroll_accum: f32,
    pub(in crate::editor) piano_beat_start: i128,
    pub(in crate::editor) piano_beat_span: hex_project::BeatTime,
    pub(in crate::editor) piano_pitch_low: u8,
    piano_pitch_rows: u8,
    piano_pitch_zoom: Option<midi_pitch_zoom::Zoom>,
    pub(in crate::editor) midi_snap_grid: midi_drag::MidiGrid,
    pending_requests: VecDeque<Request>,
    device_gesture: hex_session::device_gesture::GestureClient,
    device_gesture_barriers: VecDeque<Request>,
    effect_bus_barrier: device_gesture::EffectBusBarrierState,
    device_panel: device_panel::DevicePanelState,
    project_spectrum: project_spectrum::ProjectSpectrumState,
    sample_recorder: sample_recorder::Recorder,
    worker_requests_pending: usize,
    sampler_load: sampler_load::State,
    sampler_parameter: sampler_parameter::State,
    audio_batch: Option<Arc<hex_session::import_batch_decode::AudioBatchControl>>,
    audio_batch_progress: Option<hex_session::import_batch_decode::AudioBatchProgress>,
    pending_playback: Option<PendingPlayback>,
    pending_resident_publication: Option<PendingResidentPublication>,
    pending_audio_commands: VecDeque<PendingPlaybackAction>,
    pending_seek_ticket: Option<u64>,
    pending_metronome_ticket: Option<u64>,
    pending_session_ticket: Option<(u64, u64)>,
    viewport: Option<Viewport>,
    arranger_navigation: arranger_navigation::Extent,
    zoom_fit: zoom_fit::State,
    playback_candidates: VecDeque<PlaybackCandidate>,
    audible_playback: Option<PlaybackCandidate>,
    show_track_latency: bool,
    track_meters: track_meters::State,
    track_performance: track_performance::State,
    // One existing read-only view per transient protocol, never a prepared graph/resource set.
    resident_device_preview: Option<hex_session::service::DevicePreview>,
    resident_tuning_preview: Option<hex_session::service::TuningPreview>,
    audio_admission: AudioAdmissionStatus,
    session_routes: session_route_view::RouteView,
    prepared_seek: Option<arrangement_seek::PendingSeek>,
    seek_request_serial: u64,
    latched_seek_intent: Option<arrangement_seek::SeekIntent>,
    retired_seek_result: Option<hex_session::arrangement_seek::ArrangementSeekResult>,
    applied_project_revision: Option<u64>,
    observed_rejected_action: u64,
    observed_rejected_revision: u64,
    observed_session_capability: Option<u32>,
    session_capability_message: Option<String>,
    play_intent: play_intent::Tracker,
    opening_project: bool,
    project_open: dialogs::project_open::State,
    pending_open_path: Option<std::path::PathBuf>,
    pending_open_mode: Option<hex_session::DocumentAudioMode>,
    requested_audio_mode: Option<hex_session::DocumentAudioMode>,
    duplicate_intent: DuplicateIntent,
    pending_track_clipboard: Option<track_clipboard::Pending>,
    // Input context only: a committed cut can remove its focused header.
    // The next real selection intent or document generation invalidates it.
    track_clipboard_context: Option<(u64, u64)>,
    pending_scene_duplicate: Option<PendingSceneDuplicate>,
    pending_group_copy: Option<group_copy::Pending>,
    group_selection_history: group_selection::SelectionHistory,
    pending_clip_move: Option<pending_clip_move::Pending>,
    close_prompt_open: bool,
    export_pending: bool,
    bounce_control: Option<hex_session::service::ExportControl>,
    /// `b` is held during the active Arrangement clip drag; release bounces.
    drag_bounce_held: bool,
    pending_bounce_drop: Option<bounce::PendingBounceDrop>,
    pending_plugin_effect: Option<plugin_effect::PendingPluginEffect>,
    export_control: Option<hex_session::service::ExportControl>,
    export_window: Option<gpui::AnyWindowHandle>,
    export_dialog_open: bool,
    saving_project: bool,
    update_flow: update_flow::State,
    save_dialog_open: bool,
    document_format_upgrade_required: bool,
    format_upgrade_prompt: Option<save::FormatUpgradeIntent>,
    preset_dialog_open: bool,
    status: String,
    pub(in crate::editor) error: Option<display_error::DisplayError>,
    stream_source_warning: bool,
    recovery_failure: Option<Arc<hex_session::service::RecoveryFailure>>,
    recovery_prompt_open: bool,
    computer_keyboard: computer_keyboard::ComputerKeyboard,
    controller_input: controller_input::ControllerInput,
    _computer_keyboard_keys: gpui::Subscription,
    audio_event_editor: audio_event_editor::State,
    audio_event_focus: FocusHandle,
    _audio_event_blur: gpui::Subscription,
    focus: FocusHandle,
    pub(in crate::editor) midi_focus: FocusHandle,
    _midi_focus_lifecycle: [gpui::Subscription; 3],
    _scrub_focus_lifecycle: gpui::Subscription,
    track_scroll: track_layout::TrackScroll,
    track_layout: track_layout::TrackLayout,
    arrangement_focus: arrangement_focus::State,
    track_hierarchy_indent: track_hierarchy_indent::HierarchyIndent,
    track_reorder: track_reorder::TrackReorder,
    #[cfg(all(test, feature = "ui-test-support"))]
    track_row_builds: usize,
    focused_return: Option<hex_project::ReturnId>,
    send_menu_track: Option<TrackId>,
    pub(in crate::editor) session_scene_scroll: gpui::UniformListScrollHandle,
    redraw_gate: RedrawGate,
    cpu_meter: cpu_meter::CpuMeter,
    output_meter_frame: Option<refresh::AudioFrame>,
    disk_overload: disk_overload::DiskOverload,
    pub(in crate::editor) project_generation: u64,
    clip_view_open: bool,
    device_panel_active: bool,
    clip_detail_body_height: f32,
    clip_detail_resize: clip_detail_resize::State,
    loop_rail_gesture: Option<loop_rail::LoopRailGesture>,
    pending_loop: Option<loop_rail::PendingLoop>,
    loop_rail_cursor: gpui::CursorStyle,
    clip_content_cache: clip_content_cache::TrackCache,
    drag_lanes: std::collections::HashMap<TrackId, drag::LaneBounds>,
    track_drop_lanes: std::collections::HashMap<TrackId, drag::LaneBounds>,
    drag_lane_epochs: std::collections::HashMap<TrackId, u64>,
    arrangement_bounds: Option<drag::LaneBounds>,
    arrangement_bounds_epoch: u64,
    main_track_lane_bounds: Option<drag::LaneBounds>,
    main_track_lane_epoch: u64,
    drag_layout_generation: u64,
    arrangement_pan: Option<ArrangementPan>,
    scrub: scrub::ScrubState,
    zoom_wheel_accum: f32,
    arrangement_map_cache: std::cell::RefCell<Option<Rc<arrangement_map::MapCache>>>,
    group_overview_cache: std::cell::RefCell<group_overview::Cache>,
    audio_point_page: usize,
    overview_waveform_cache: Rc<std::cell::RefCell<overview_waveform::Cache>>,
    pub(in crate::editor) waveform_geometry_cache:
        Rc<std::cell::RefCell<waveform_geometry::WaveformGeometryCache>>,
    midi_visible_notes: std::cell::RefCell<midi_loop_notes::VisibleNotesCache>,
    midi_tile_images: Rc<std::cell::RefCell<midi_loop_notes::TileImages>>,
    active_clip_drag: Option<drag::ClipDrag>,
    active_browser_audio_drag: Option<drag::BrowserAudioDrag>,
    pending_sample_drop: Option<pending_sample_drop::PendingSampleDrop>,
    arrangement_device_preview: Option<arrangement_device_drop::ArrangementDevicePreview>,
    active_native_device_drag: Option<drag::NativeDeviceDrag>,
    active_trim_drag: Option<trim::TrimDrag>,
    pending_trim: Option<trim::PendingTrim>,
    fade_key_held: bool,
    /// Option held: the right clip header edge offers repeat-extend.
    option_held: bool,
    time_selection_key_held: bool,
    active_fade_drag: Option<fade_handles::FadeDrag>,
    pending_fade: Option<fade_handles::PendingFade>,
    clip_gain: clip_gain::State,
    hovered_fade_clip: Option<(u64, ClipId)>,
    hovered_fade_handle: Option<fade_handles::Hover>,
    active_midi_trim_drag: Option<trim::MidiTrimDrag>,
    pointer_down: Option<drag::PointerDown>,
    #[cfg(test)]
    drag_move_event_count: usize,
    #[cfg(test)]
    last_enqueued_edit: Option<Command>,
    #[cfg(test)]
    last_enqueued_clip_conversion: Option<(u64, u64, ClipId, bool)>,
    #[cfg(test)]
    last_enqueued_native_device_add: Option<(
        TrackId,
        hex_engine::NativeDeviceClass,
        hex_lattice_contract::DeviceTypeId,
    )>,
    #[cfg(test)]
    last_enqueued_native_device_replace: Option<(
        hex_project::DeviceId,
        hex_engine::NativeDeviceClass,
        hex_lattice_contract::DeviceTypeId,
    )>,
    #[cfg(test)]
    last_enqueued_arrangement_device_drop:
        Option<hex_session::service::ArrangementDeviceDestination>,
    #[cfg(test)]
    last_enqueued_device_track_gain: Option<Option<f32>>,
    #[cfg(test)]
    last_enqueued_native_device_drop: Option<EnqueuedNativeDeviceDrop>,
    #[cfg(test)]
    last_time_request: Option<(&'static str, Option<(TrackId, u64)>)>,
    #[cfg(test)]
    last_transport_command: Option<PlaybackCommand>,
    #[cfg(test)]
    last_trim_release_snap: Option<bool>,
    #[cfg(test)]
    last_enqueued_import_to_track: Option<(std::path::PathBuf, TrackId, u64, bool)>,
    #[cfg(test)]
    last_enqueued_import_to_new_track: Option<(std::path::PathBuf, Option<TrackId>, u64)>,
    snap_grid: musical_time::SnapGrid,
    input_quantization: hex_project::InputQuantization,
    #[cfg(test)]
    test_environment: Arc<test_environment::Environment>,
    browser: browser::BrowserState,
    recent_projects: RecentProjectsState,
    project_directory: project_directory::State,
    changelog: changelog::State,
    missing_media: Option<missing_media::Panel>,
    theme_settings: theme_settings::ThemeSettingsState,
    project_rate_settings: project_rate_settings::State,
    audio_import_settings: audio_import_settings::State,
    track_defaults: track_defaults::State,
    tuning: tuning::TuningState,
    context_menu: Option<crate::context_menu::PopupMenu<context_menu::EditorContextAction>>,
    edit_cursor: Option<edit_cursor::EditCursor>,
    /// Transient Arrangement play-start lock; never seeks the active transport.
    locked_play_start: Option<edit_cursor::EditCursor>,
    /// The selected clip a point cursor was deliberately placed inside; that
    /// cursor then outranks the clip's play start (see `selected_arrangement_play_start`).
    edit_cursor_in_selected_clip: Option<ClipId>,
    /// The selected clip whose in-clip cursor a loop enable retired; the loop
    /// start then keeps outranking that clip's play start until a new click.
    loop_start_over_clip: Option<ClipId>,
    /// Successful selection-loop start, fenced by the user's selection intent.
    selection_loop_start: Option<(u64, hex_project::LoopRegion)>,
    detail_context: Option<detail_context::DetailContext>,
    clip_paint_gesture: Option<clip_paint::ClipPaintGesture>,
    time_selection: Option<time_selection::TimeSelection>,
    time_selection_project_scope: bool,
    selection_intent_epoch: Option<u64>,
    time_selection_gesture: Option<time_selection::TimeSelectionGesture>,
    time_range_move_gesture: Option<time_selection::TimeRangeMoveGesture>,
    pending_time_range_move: Option<time_selection::PendingTimeRangeMove>,
    suppress_cursor_click: bool,
}

impl EditorView {
    /// Modal operations retain project selections but must not delete them.
    fn selection_delete_blocked(&self) -> bool {
        self.opening_project
            || self.close_prompt_open
            || self.changelog.is_open()
            || self.save_dialog_open
            || self.export_dialog_open
            || self.preset_dialog_open
            || self.tuning.open
            || self.theme_settings.open
            || self.context_menu.is_some()
            || self.controller_input_is_open()
            || self.send_menu_track.is_some()
            || (self.clip_view_open
                && self.device_panel_active
                && self.device_panel.add_menu_is_open(self.state.as_deref()))
    }

    fn new_with_backend(
        window: &mut Window,
        cx: &mut Context<Self>,
        backend: crate::EditorBackend,
    ) -> Self {
        let mut view = Self::new(
            window,
            cx,
            backend.service,
            Err("preparing audio output".into()),
        );
        view.audio_events = backend.audio_events;
        view.source_reservation = Some(backend.source_reservation);
        view.status = "preparing audio output".into();
        // A session that ended unexpectedly may have left untitled work.
        view.check_leftover_recovery(None);
        view
    }

    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        service: Service,
        output: Result<crate::EditorAudio, String>,
    ) -> Self {
        #[cfg(not(test))]
        {
            Self::new_with_browser_preferences_path(
                window,
                cx,
                service,
                output,
                browser::default_preferences_path(),
                recent_projects::default_preferences_path(),
                theme_settings::default_preferences_path(),
            )
        }
        #[cfg(test)]
        {
            Self::new_with_browser_paths(
                window,
                cx,
                service,
                output,
                (
                    std::path::PathBuf::new(),
                    std::path::PathBuf::new(),
                    std::path::PathBuf::new(),
                    std::path::PathBuf::new(),
                ),
                #[cfg(feature = "ui-test-support")]
                None,
                test_environment::Inputs {
                    defaults: [true; 4],
                    ..Default::default()
                },
            )
        }
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    fn new_for_test(
        window: &mut Window,
        cx: &mut Context<Self>,
        service: Service,
        output: Result<crate::EditorAudio, String>,
        preferences_path: std::path::PathBuf,
        recent_projects_path: std::path::PathBuf,
    ) -> Self {
        Self::new_with_browser_paths(
            window,
            cx,
            service,
            output,
            (
                preferences_path,
                recent_projects_path,
                std::path::PathBuf::new(),
                std::path::PathBuf::new(),
            ),
            #[cfg(feature = "ui-test-support")]
            None,
            test_environment::Inputs {
                defaults: [false, false, true, true],
                ..Default::default()
            },
        )
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    fn new_for_test_with_theme_preferences_path(
        window: &mut Window,
        cx: &mut Context<Self>,
        service: Service,
        output: Result<crate::EditorAudio, String>,
        preferences_path: std::path::PathBuf,
        recent_projects_path: std::path::PathBuf,
        theme_preferences_path: std::path::PathBuf,
    ) -> Self {
        Self::new_with_browser_preferences_path(
            window,
            cx,
            service,
            output,
            preferences_path,
            recent_projects_path,
            theme_preferences_path,
        )
    }

    /// Bound all filesystem discovery before any worker starts.
    #[cfg(all(test, feature = "ui-test-support"))]
    fn new_for_test_with_browser_directory(
        window: &mut Window,
        cx: &mut Context<Self>,
        service: Service,
        directory: &std::path::Path,
    ) -> Self {
        Self::new_for_test_with_project_preferences(
            window,
            cx,
            service,
            directory,
            directory.join("project-directory.json"),
        )
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    fn new_for_test_with_project_preferences(
        window: &mut Window,
        cx: &mut Context<Self>,
        service: Service,
        directory: &std::path::Path,
        project_preferences: std::path::PathBuf,
    ) -> Self {
        Self::new_with_browser_paths(
            window,
            cx,
            service,
            Err("offline ui test".into()),
            (
                directory.join("browser.json"),
                directory.join("recent-projects.json"),
                directory.join("theme.json"),
                project_preferences,
            ),
            #[cfg(feature = "ui-test-support")]
            None,
            test_environment::Inputs {
                directory: Some(directory.join("samples")),
                ..Default::default()
            },
        )
    }

    /// Reuse the fixture's actual path owner before any startup work escapes.
    #[cfg(all(test, feature = "ui-test-support"))]
    fn new_for_test_with_environment(
        window: &mut Window,
        cx: &mut Context<Self>,
        service: Service,
        environment: Arc<test_environment::Environment>,
    ) -> Self {
        let directory = environment.root().join("samples");
        Self::new_with_browser_paths(
            window,
            cx,
            service,
            Err("offline ui test".into()),
            (
                std::path::PathBuf::new(),
                std::path::PathBuf::new(),
                std::path::PathBuf::new(),
                std::path::PathBuf::new(),
            ),
            #[cfg(feature = "ui-test-support")]
            None,
            test_environment::Inputs {
                existing_environment: Some(environment),
                directory: Some(directory),
                defaults: [true; 4],
                ..Default::default()
            },
        )
    }

    fn new_with_browser_preferences_path(
        window: &mut Window,
        cx: &mut Context<Self>,
        service: Service,
        output: Result<crate::EditorAudio, String>,
        preferences_path: std::path::PathBuf,
        recent_projects_path: std::path::PathBuf,
        theme_preferences_path: std::path::PathBuf,
    ) -> Self {
        Self::new_with_browser_paths(
            window,
            cx,
            service,
            output,
            (
                preferences_path,
                recent_projects_path,
                theme_preferences_path,
                project_directory::default_preferences_path(),
            ),
            #[cfg(feature = "ui-test-support")]
            None,
            #[cfg(test)]
            test_environment::Inputs {
                defaults: [false, false, false, true],
                ..Default::default()
            },
        )
    }

    fn new_with_browser_paths(
        window: &mut Window,
        cx: &mut Context<Self>,
        service: Service,
        output: Result<crate::EditorAudio, String>,
        paths: (
            std::path::PathBuf,
            std::path::PathBuf,
            std::path::PathBuf,
            std::path::PathBuf,
        ),
        #[cfg(feature = "ui-test-support")] fixture_paths: Option<sampler_load::FixturePaths>,
        #[cfg(test)] test_paths: test_environment::Inputs,
    ) -> Self {
        #[cfg(all(test, feature = "ui-test-support"))]
        cx.use_test_assets(crate::assets::HexAssets);
        crate::diagnostics::attach_service(cx, &service);
        #[cfg(test)]
        let test_environment = test_paths.existing_environment.unwrap_or_else(|| {
            test_environment::Environment::new()
                .expect("could not create private editor test environment; no ambient fallback")
        });
        #[cfg(test)]
        let paths = test_environment.resolve_paths(paths, test_paths.defaults);

        let (
            preferences_path,
            recent_projects_path,
            theme_preferences_path,
            project_directory_path,
        ) = paths;
        let (audio, status) = match output {
            Ok(audio) => (Some(audio), "loading project · audio ready".into()),
            Err(error) => (
                None,
                format!(
                    "audio unavailable · offline editing · {}",
                    error.to_lowercase()
                ),
            ),
        };
        let poll_cadence = Self::spawn_poll(window, cx);
        let keyboard_window_id = window.window_handle().window_id();
        let keyboard_entity = cx.entity().downgrade();
        let computer_keyboard_keys = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle().window_id() != keyboard_window_id {
                return;
            }
            crate::diagnostics::record_key(cx, event);
            if let Some(entity) = keyboard_entity.upgrade() {
                entity.update(cx, |view, _| view.wake_poll());
            }
            // A key during the potentially slow helper handshake cancels the
            // pending quit before the key can alter a project or local draft.
            if let Some(entity) = keyboard_entity.upgrade()
                && entity.read(cx).update_preparing()
            {
                entity.update(cx, |view, cx| view.cancel_requested_update(cx));
            }
            // A completed drag can leave this window with no mounted editor
            // focus. Restore it and deliver the first Space press here;
            // subsequent keys, including repeats, take the normal raw-key path.
            if event.keystroke.key == "space"
                && event.keystroke.modifiers == gpui::Modifiers::default()
                && let Some(entity) = keyboard_entity.upgrade()
                && !entity.read(cx).focus.contains_focused(window, cx)
                && !event.context_stack.iter().any(|context| {
                    context.contains("hex_text_editing")
                        || context.contains("hex_text_input")
                        || context.contains("hex_browser_search")
                })
            {
                entity.update(cx, |view, cx| {
                    view.focus.focus(window);
                    let _scope = crate::diagnostics::action_scope(cx, "hex_editor::TogglePlay");
                    view.toggle_play();
                    cx.notify();
                });
                cx.stop_propagation();
                return;
            }
            if window.window_handle().window_id() == keyboard_window_id
                && let Some(entity) = keyboard_entity.upgrade()
                && (entity.read(cx).tuning_audition_accepts(
                    &event.keystroke.key,
                    event.keystroke.modifiers,
                    window,
                ) || entity.read(cx).computer_keyboard_accepts(
                    &event.keystroke.key,
                    event.keystroke.modifiers,
                    window,
                    cx,
                ))
            {
                cx.stop_propagation();
            }
        });
        let focus = cx.focus_handle();
        let scrub_focus_blur = cx.on_blur(&focus, window, |view, window, cx| {
            view.clear_time_selection_key_on_blur(window, cx);
            if !view.focus.is_focused(window) {
                view.cancel_fade_gesture(cx);
                view.cancel_clip_gain(cx);
                view.cancel_project_spectrum_resize(cx);
                view.cancel_clip_detail_resize(cx);
                if view.cancel_trim_gesture(cx) {
                    cx.stop_active_drag(window);
                    // Blur runs after paint in GPUI's focus phase. Notify again
                    // through the normal effect queue so the canceled overlay
                    // is removed in the next frame, without another input.
                    cx.defer_in(window, |_, _, cx| cx.notify());
                }
                view.cancel_track_resize(cx);
            }
            view.cancel_computer_keyboard();
            view.computer_keyboard.suspended = true;
            view.midi_step.cancel_pending();
            view.invalidate_hardware_step();
            if view.cancel_loop_rail_gesture() {
                cx.defer_in(window, |_, _, cx| cx.notify());
            }
            if view.cancel_scrub() | view.cancel_clip_paint() | view.cancel_section_pointer() {
                cx.notify();
            }
        });
        let audio_event_focus = cx.focus_handle();
        let audio_event_blur = cx.on_blur(&audio_event_focus, window, |view, window, cx| {
            view.clear_time_selection_key_on_blur(window, cx);
            view.audio_event_editor.gesture = None;
            view.cancel_audio_loop_gesture();
            view.audio_event_editor.fade_key_held = false;
            cx.notify();
        });
        let midi_focus = cx.focus_handle();
        let focus_blur = cx.on_blur(&midi_focus, window, |view, window, cx| {
            view.clear_time_selection_key_on_blur(window, cx);
            view.cancel_computer_keyboard();
            view.computer_keyboard.suspended = true;
            view.midi_step.cancel_pending();
            view.invalidate_hardware_step();
            let midi_cancelled = view.cancel_midi_note_gesture();
            if midi_cancelled {
                // Focus listeners run after frame swap; invalidate once the
                // normal effect queue resumes so stationary previews disappear.
                cx.defer_in(window, |_, _, cx| cx.notify());
            }
        });
        let window_activation = cx.observe_window_activation(window, |view, window, cx| {
            if !window.is_window_active() {
                view.time_selection_key_held = false;
                let boundary_cancelled = device_panel::cancel_sampler_boundary(view);
                let browser_cancelled = view.cancel_active_browser_audio_drag();
                view.cancel_tuning_audition();
                view.cancel_fade_gesture(cx);
                view.cancel_clip_gain(cx);
                view.cancel_project_spectrum_resize(cx);
                view.cancel_clip_detail_resize(cx);
                if view.cancel_trim_gesture(cx) {
                    cx.stop_active_drag(window);
                }
                view.cancel_track_resize(cx);
                view.cancel_computer_keyboard();
                view.computer_keyboard.suspended = true;
                view.midi_step.cancel_pending();
                view.invalidate_hardware_step();
                view.audio_event_editor.gesture = None;
                view.audio_event_editor.fade_key_held = false;
                let midi_cancelled = view.cancel_midi_note_gesture();
                let scrub_cancelled = view.cancel_scrub();
                let paint_cancelled = view.cancel_audio_loop_gesture()
                    | view.cancel_clip_paint()
                    | view.cancel_section_pointer()
                    | view.cancel_track_reorder();
                if let Some(audio) = view.audio.as_mut() {
                    let _ = audio.control.audition_cancel();
                }
                device_panel::clear_eq8_audition_gesture_flags(view);
                if boundary_cancelled
                    || browser_cancelled
                    || midi_cancelled
                    || scrub_cancelled
                    || paint_cancelled
                {
                    cx.notify();
                }
            }
        });
        let window_bounds = cx.observe_window_bounds(window, |view, window, cx| {
            device_panel::cancel_sampler_boundary(view);
            view.cancel_fade_gesture(cx);
            view.cancel_clip_gain(cx);
            view.cancel_project_spectrum_resize(cx);
            view.cancel_clip_detail_resize(cx);
            if view.cancel_trim_gesture(cx) {
                cx.stop_active_drag(window);
            }
            view.cancel_track_resize(cx);
            view.audio_event_editor.gesture = None;
            view.audio_event_editor.fade_key_held = false;
            if view.cancel_midi_note_gesture()
                | view.cancel_audio_loop_gesture()
                | view.cancel_track_reorder()
                | view.cancel_scrub()
                | view.cancel_clip_paint()
                | view.cancel_section_pointer()
            {
                cx.notify();
            }
        });
        focus.focus(window);
        let browser = browser::BrowserState::new(
            cx,
            preferences_path,
            #[cfg(test)]
            Some(
                test_paths
                    .home
                    .unwrap_or_else(|| test_environment.home.clone()),
            ),
        );
        let recent_projects = RecentProjectsState::new(recent_projects_path);
        let project_directory = project_directory::State::new(project_directory_path);
        let audio_import_settings = audio_import_settings::State::new(
            theme_preferences_path.with_file_name("audio-import.json"),
        );
        let track_defaults = track_defaults::State::new(
            theme_preferences_path.with_file_name("track-defaults.json"),
        );
        let controller_input = controller_input::ControllerInput::new(
            theme_preferences_path.with_file_name("midi-input.json"),
        );
        let theme_settings =
            theme_settings::ThemeSettingsState::new(theme_preferences_path, window, cx);
        let mut view = Self {
            transport_trace: crate::diagnostics::TransportTrace::from_app(cx),
            service,
            solo_observation: None,
            audio,
            retired_action_outcomes: None,
            retired_action_message: None,
            audio_events: std::sync::mpsc::channel().1,
            source_reservation: None,
            restart_request_id: 0,
            pending_restart_request: None,
            pending_recovery_generation: None,
            route_follow: None,
            degraded_plugins: Default::default(),
            pending_audio_result: None,
            pending_publication_observation: None,
            document_audio_mode: hex_session::DocumentAudioMode::Resident,
            state: None,
            recovery: RecoveryState::default(),
            stopped_playback_restore: None,
            latency_rebuild: None,
            leftover_recovery: Default::default(),
            truncated_import: Default::default(),
            sampler_edit_resume: None,
            deferred_play: None,
            offline_cursor: 0,
            selected_track: None,
            track_selection: selection::TrackSelection::default(),
            gain_lane: gain_lane::GainLane::default(),
            automation_view: automation_view::AutomationView::default(),
            audio_clip_controls: Default::default(),
            sample_bpm_control: None,
            project_tempo_control: None,
            track_gain_controls: track_gain::Controls::default(),
            gain_recorder: gain_recording::Recorder::default(),
            native_automation_recorder: native_automation_recording::Recorder::default(),
            midi_recorder: midi_recording::Recorder::default(),
            routing_open: false,
            routing_page: 0,
            selected_clip: None,
            clip_selection: clip_selection::ClipSelection::default(),
            clip_name_edit: None,
            sections: arrangement_sections::Sections::default(),
            section_autoplay: false,
            track_name: track_name::TrackNameState::default(),
            track_tags: track_tags::State::default(),
            mode: EditorMode::Arrangement,
            selected_scene: None,
            selected_cell: None,
            session_copy_source: None,
            session_track_start: 0,
            console_returns: console_returns::State::default(),
            session_visible_columns: 1,
            session_quantization: hex_engine::LaunchQuantization::Immediate,
            midi_selection: midi_selection::MidiSelection::default(),
            midi_note_gesture: None,
            midi_group_gesture: None,
            pending_midi_drag: None,
            midi_origin_history: midi_origin_view::State::default(),
            midi_creation: None,
            midi_stroke: None,
            midi_draw_mode: false,
            midi_draw_pitch_locked: true,
            midi_marker: None,
            midi_step: midi_step::StepEntry::default(),
            midi_velocity: midi_velocity_ui::State::default(),
            midi_velocity_randomize: midi_velocity_randomize::State::default(),
            midi_pan: midi_pan::State::default(),
            midi_ruler: midi_ruler::State::default(),
            clip_loop: clip_loop::State::default(),
            midi_time_selection: midi_ruler::TimeSelection::default(),
            midi_audition: midi_audition::MidiAudition::default(),
            pending_created_note_selection: None,
            midi_marquee: None,
            midi_scene_cache: midi_scene::MidiSceneCache::default(),
            midi_hit_cycle: None,
            midi_hover: None,
            piano_view_clip: None,
            piano_zoom_wheel_accum: 0.0,
            piano_pitch_wheel_accum: 0.0,
            poll_cadence,
            piano_pitch_scroll_accum: 0.0,
            piano_beat_start: 0,
            piano_beat_span: hex_project::BeatTime::from_quarters(4)
                .unwrap_or(hex_project::BeatTime::ONE),
            piano_pitch_low: 48,
            piano_pitch_rows: 24,
            piano_pitch_zoom: None,
            midi_snap_grid: midi_drag::MidiGrid::default(),
            pending_requests: VecDeque::new(),
            device_gesture: hex_session::device_gesture::GestureClient::default(),
            device_gesture_barriers: VecDeque::new(),
            effect_bus_barrier: Default::default(),
            device_panel: device_panel::DevicePanelState::default(),
            project_spectrum: Default::default(),
            sample_recorder: Default::default(),
            worker_requests_pending: 0,
            sampler_load: Default::default(),
            sampler_parameter: Default::default(),
            audio_batch: None,
            audio_batch_progress: None,
            pending_playback: None,
            pending_resident_publication: None,
            pending_audio_commands: VecDeque::new(),
            pending_seek_ticket: None,
            pending_metronome_ticket: None,
            pending_session_ticket: None,
            viewport: None,
            arranger_navigation: arranger_navigation::Extent::default(),
            zoom_fit: zoom_fit::State::default(),
            playback_candidates: VecDeque::new(),
            audible_playback: None,
            show_track_latency: false,
            track_meters: Default::default(),
            track_performance: Default::default(),
            resident_device_preview: None,
            resident_tuning_preview: None,
            audio_admission: AudioAdmissionStatus::default(),
            session_routes: session_route_view::RouteView::default(),
            prepared_seek: None,
            seek_request_serial: 0,
            latched_seek_intent: None,
            retired_seek_result: None,
            applied_project_revision: None,
            observed_rejected_action: 0,
            observed_rejected_revision: 0,
            observed_session_capability: None,
            session_capability_message: None,
            play_intent: play_intent::Tracker::default(),
            opening_project: false,
            project_open: dialogs::project_open::State::default(),
            pending_open_path: None,
            pending_open_mode: None,
            requested_audio_mode: None,
            duplicate_intent: DuplicateIntent::default(),
            pending_track_clipboard: None,
            track_clipboard_context: None,
            pending_scene_duplicate: None,
            pending_group_copy: None,
            group_selection_history: group_selection::SelectionHistory::default(),
            pending_clip_move: None,
            close_prompt_open: false,
            export_pending: false,
            bounce_control: None,
            drag_bounce_held: false,
            pending_bounce_drop: None,
            pending_plugin_effect: None,
            export_control: None,
            export_window: None,
            export_dialog_open: false,
            saving_project: false,
            update_flow: update_flow::State::default(),
            save_dialog_open: false,
            document_format_upgrade_required: false,
            format_upgrade_prompt: None,
            preset_dialog_open: false,
            status,
            error: None,
            stream_source_warning: false,
            recovery_failure: None,
            recovery_prompt_open: false,
            computer_keyboard: computer_keyboard::ComputerKeyboard::default(),
            controller_input,
            _computer_keyboard_keys: computer_keyboard_keys,
            focus,
            audio_event_editor: audio_event_editor::State::default(),
            audio_event_focus,
            _audio_event_blur: audio_event_blur,
            midi_focus,
            _midi_focus_lifecycle: [focus_blur, window_activation, window_bounds],
            _scrub_focus_lifecycle: scrub_focus_blur,
            track_scroll: track_layout::TrackScroll::new(),
            track_layout: track_layout::TrackLayout::default(),
            arrangement_focus: arrangement_focus::State::default(),
            track_hierarchy_indent: track_hierarchy_indent::HierarchyIndent::default(),
            track_reorder: track_reorder::TrackReorder::default(),
            #[cfg(all(test, feature = "ui-test-support"))]
            track_row_builds: 0,
            focused_return: None,
            send_menu_track: None,
            session_scene_scroll: gpui::UniformListScrollHandle::new(),
            redraw_gate: RedrawGate::default(),
            cpu_meter: cpu_meter::CpuMeter::default(),
            output_meter_frame: None,
            disk_overload: disk_overload::DiskOverload::default(),
            project_generation: 0,
            clip_view_open: true,
            device_panel_active: false,
            clip_detail_body_height: clip_view::CLIP_DETAIL_BODY_HEIGHT,
            clip_detail_resize: clip_detail_resize::State::default(),
            loop_rail_gesture: None,
            pending_loop: None,
            loop_rail_cursor: gpui::CursorStyle::Crosshair,
            clip_content_cache: clip_content_cache::TrackCache::default(),
            drag_lanes: std::collections::HashMap::new(),
            track_drop_lanes: std::collections::HashMap::new(),
            drag_lane_epochs: std::collections::HashMap::new(),
            arrangement_bounds: None,
            arrangement_bounds_epoch: 0,
            main_track_lane_bounds: None,
            main_track_lane_epoch: 0,
            drag_layout_generation: 0,
            arrangement_pan: None,
            scrub: scrub::ScrubState::default(),
            zoom_wheel_accum: 0.0,
            arrangement_map_cache: std::cell::RefCell::new(None),
            group_overview_cache: std::cell::RefCell::default(),
            audio_point_page: 0,
            overview_waveform_cache: Rc::new(std::cell::RefCell::new(
                overview_waveform::Cache::default(),
            )),
            waveform_geometry_cache: Rc::new(std::cell::RefCell::new(
                waveform_geometry::WaveformGeometryCache::default(),
            )),
            midi_visible_notes: Default::default(),
            midi_tile_images: Default::default(),
            active_clip_drag: None,
            active_browser_audio_drag: None,
            pending_sample_drop: None,
            active_native_device_drag: None,
            arrangement_device_preview: None,
            active_trim_drag: None,
            pending_trim: None,
            fade_key_held: false,
            option_held: false,
            time_selection_key_held: false,
            active_fade_drag: None,
            pending_fade: None,
            clip_gain: clip_gain::State::default(),
            hovered_fade_clip: None,
            hovered_fade_handle: None,
            active_midi_trim_drag: None,
            pointer_down: None,
            #[cfg(test)]
            drag_move_event_count: 0,
            #[cfg(test)]
            last_enqueued_edit: None,
            #[cfg(test)]
            last_enqueued_clip_conversion: None,
            #[cfg(test)]
            last_enqueued_native_device_add: None,
            #[cfg(test)]
            last_enqueued_native_device_replace: None,
            #[cfg(test)]
            last_enqueued_native_device_drop: None,
            #[cfg(test)]
            last_enqueued_arrangement_device_drop: None,
            #[cfg(test)]
            last_enqueued_device_track_gain: None,
            #[cfg(test)]
            last_time_request: None,
            #[cfg(test)]
            last_transport_command: None,
            #[cfg(test)]
            last_trim_release_snap: None,
            #[cfg(test)]
            last_enqueued_import_to_track: None,
            #[cfg(test)]
            last_enqueued_import_to_new_track: None,
            snap_grid: musical_time::SnapGrid::default(),
            input_quantization: hex_project::InputQuantization::Off,
            #[cfg(test)]
            test_environment,
            browser,
            recent_projects,
            project_directory,
            changelog: changelog::State::new(cx),
            missing_media: None,
            theme_settings,
            project_rate_settings: project_rate_settings::State::default(),
            audio_import_settings,
            track_defaults,
            tuning: tuning::TuningState::new(cx),
            context_menu: None,
            edit_cursor: None,
            locked_play_start: None,
            edit_cursor_in_selected_clip: None,
            loop_start_over_clip: None,
            selection_loop_start: None,
            detail_context: None,
            clip_paint_gesture: None,
            time_selection: None,
            time_selection_project_scope: false,
            selection_intent_epoch: Some(0),
            time_selection_gesture: None,
            time_range_move_gesture: None,
            pending_time_range_move: None,
            suppress_cursor_click: false,
        };
        #[cfg(test)]
        if let Some(directory) = test_paths.directory {
            view.browser.directory = directory.clone();
            view.browser.places = vec![directory];
        }
        #[cfg(feature = "ui-test-support")]
        if let Some(paths) = fixture_paths {
            view.browser
                .set_native_sampler_fixture_paths(paths.home, paths.directory);
        }
        view.browser_scan_initial(cx);
        view.start_recent_projects_load(cx);
        view.start_project_directory_load(cx);
        view.start_theme_preferences_load(cx);
        view.start_audio_import_preferences_load(cx);
        view.start_track_defaults_load(cx);
        view.start_controller_preferences_load(cx);
        view
    }

    fn selected(&self) -> Option<(TrackId, ClipId)> {
        Some((self.selected_track?, self.selected_clip?))
    }

    pub(super) fn selected_clip_data(&self) -> Option<(TrackId, ClipId, u64, u64, u64, u64)> {
        let (track_id, clip_id) = self.selected()?;
        let state = self.state.as_ref()?;
        let track = state.project.track(track_id)?;
        let clip = track.clips().iter().find(|clip| clip.id() == clip_id)?;
        Some((
            track_id,
            clip_id,
            clip.start(),
            clip.duration(),
            clip.source_offset(),
            clip.source_length(),
        ))
    }

    pub(super) fn select_track(&mut self, track: TrackId, window: &mut Window) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if self
            .track_selection
            .select(&state.project, track, false, false)
        {
            self.focus_selected_track(track, window);
        }
    }

    pub(super) fn focus_selected_track(&mut self, track: TrackId, window: &mut Window) {
        self.advance_selection_intent();
        let header_focused = self.track_header_focused(window);
        self.duplicate_intent.cancel_chain();
        self.midi_note_gesture = None;
        self.midi_hit_cycle = None;
        self.focus_track_details(window);
        self.selected_track = Some(track);
        self.arrangement_focus.observe_active_track(Some(track));
        if let Some(state) = self.state.as_ref() {
            self.gain_lane.change_owner(track, &state.project);
        }
        self.focused_return = None;
        self.selected_clip = None;
        self.midi_selection.clear();
        self.update_track_selection_status();
        if header_focused {
            self.focus_track_header_after_navigation(track, window);
        }
    }

    pub(super) fn toggle_play(&mut self) {
        if self.section_install_pending() {
            self.error = Some("applying edit · wait for audio".into());
            return;
        }
        if self.opening_project || self.close_prompt_open {
            self.error = Some("project change pending · playback is paused".into());
            return;
        }
        if self.recovery.generation().is_some() {
            self.defer_play_until_audio_recovers();
            return;
        }
        let device_playing = self.audio.as_ref().is_some_and(|audio| {
            audio
                .control
                .status()
                .is_some_and(|status| status.playing.load(std::sync::atomic::Ordering::Acquire))
        });
        let start_frame = self.playback_start_frame();
        if !self.desired_playing(device_playing) && self.recover_degraded_play_at(start_frame) {
            return;
        }
        let command = match self.play_toggle_action(device_playing, start_frame) {
            arrangement_seek::ToggleAction::Pause => PlaybackCommand::Pause,
            arrangement_seek::ToggleAction::PlayFrom(frame) => PlaybackCommand::PlayFrom(frame),
            arrangement_seek::ToggleAction::Unavailable => {
                self.error = Some("playback source is unavailable · cannot seek safely".into());
                return;
            }
        };
        #[cfg(test)]
        {
            self.last_transport_command = Some(command);
        }
        self.queue_audio_command(command);
    }

    fn playback_start_frame(&self) -> u64 {
        if let Some(cursor) = self.valid_locked_play_start() {
            return cursor.frame;
        }
        if let Some((epoch, region)) = self.selection_loop_start
            && self.selection_intent_epoch == Some(epoch)
            && self
                .state
                .as_ref()
                .and_then(|state| state.project.loop_region())
                == Some(region)
        {
            return region.start();
        }
        // The user's placed position wins, even outside an enabled loop; the
        // acknowledged canonical loop only supplies a start when none is placed.
        // Enabling the loop retires an earlier point cursor (Pulse #3838).
        // A whole-rectangle selection replaces the point cursor, not a time range.
        if let Some(selection) = self.normalized_time_selection() {
            return selection.start();
        }
        if let Some(frame) = self.selected_arrangement_play_start() {
            return frame;
        }
        if let Some(cursor) = self
            .edit_cursor
            .filter(|cursor| cursor.generation == self.project_generation)
        {
            return cursor.frame;
        }
        self.state
            .as_ref()
            .and_then(|state| state.project.loop_region())
            .filter(|region| region.enabled())
            .map_or(0, |region| region.start())
    }

    fn observe_solo_view(&mut self, view: hex_session::service::SoloViewObservation) -> bool {
        if view.project_generation != self.project_generation
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.revision != view.previous_revision)
            || view.revision != view.previous_revision.saturating_add(1)
        {
            return false;
        }
        self.solo_observation = Some(view);
        true
    }

    fn displayed_solo(&self, track: TrackId, canonical: bool) -> bool {
        self.solo_observation
            .as_ref()
            .filter(|view| {
                view.project_generation == self.project_generation
                    && self
                        .state
                        .as_ref()
                        .is_some_and(|state| state.revision == view.previous_revision)
            })
            .and_then(|view| {
                view.tracks
                    .iter()
                    .find(|(id, _)| *id == track)
                    .map(|(_, soloed)| *soloed)
            })
            .unwrap_or(canonical)
    }

    pub(super) fn toggle_solo(&mut self, track: TrackId, exclusive: bool) {
        let selected = self.selected_track_ids();
        let command = if selected.len() > 1 && selected.contains(&track) {
            match hex_project::TrackSelectionIds::new(&selected) {
                Ok(tracks) => Command::ToggleSoloTracks { tracks, exclusive },
                Err(error) => {
                    self.error = Some(error.to_string().into());
                    return;
                }
            }
        } else {
            Command::ToggleSolo { track, exclusive }
        };
        self.enqueue(Request::Edit(command));
    }

    pub(super) fn select_neighbor_track(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.opening_project || self.close_prompt_open {
            return;
        }
        let choice = if self.mode == EditorMode::Arrangement
            && (self.arrangement_focus.projection.enabled
                || self.arrangement_focus.projection.filtering())
        {
            self.focused_neighbor_track(forward)
        } else {
            self.state
                .as_ref()
                .and_then(|state| selection::neighbor(&state.project, self.selected_track, forward))
        };
        if let Some((index, track, clip)) = choice {
            self.select_track(track, window);
            if let Some(clip) = clip
                && self.focus_clip_details(window, cx)
            {
                self.selected_clip = Some(clip);
            }
            let row = if self.mode == EditorMode::Arrangement
                && (self.arrangement_focus.projection.enabled
                    || self.arrangement_focus.projection.filtering())
            {
                Some(index)
            } else {
                self.arrangement_row_index(index)
            };
            if let Some(row) = row {
                self.track_scroll
                    .scroll_to_item(row, gpui::ScrollStrategy::Top);
            }
        }
    }

    pub(super) fn toggle_mute(&mut self, track: TrackId) {
        self.enqueue(Request::Edit(Command::ToggleMute(track)));
    }

    /// Ordinary creation follows the selected row; Return/Main and an absent or
    /// stale ordinary selection append within the ordinary-track section. Drops
    /// and context menus retain their explicitly captured insertion anchors.
    fn new_track_insertion_anchor(&self) -> Option<TrackId> {
        if self.focused_return.is_some() {
            return None;
        }
        let selected = self.selected_track?;
        self.state
            .as_ref()?
            .project
            .tracks()
            .iter()
            .skip_while(|track| track.id() != selected)
            .nth(1)
            .map(|track| track.id())
    }

    pub(super) fn add_track(&mut self) {
        self.insert_default_track(self.new_track_insertion_anchor());
    }

    pub(super) fn remove_track(&mut self, cx: &mut Context<Self>) {
        if !device_panel::finalize_controls(self, cx) {
            return;
        }
        if let (Some(track), Some(state)) = (self.selected_track, self.state.as_ref())
            && state.project.track(track).is_some()
        {
            self.duplicate_intent.cancel_chain();
            self.enqueue_device_barrier(Request::Edit(Command::RemoveTrack(track)));
            self.selected_track = None;
            self.selected_clip = None;
        }
    }

    pub(super) fn state(&self) -> Option<&Arc<ViewState>> {
        self.state.as_ref()
    }

    pub(super) fn audio_status(&self) -> Option<&Status> {
        self.audio.as_ref().and_then(|audio| audio.status())
    }

    fn viewport_horizon(&self, horizon: u64) -> u64 {
        // Display all authored points while this editor is open. Canonical content/export
        // extent stays unchanged; a held gain endpoint is not an audible source duration.
        let gain_end = self.state.as_ref().and_then(|state| {
            self.gain_lane
                .track
                .and_then(|id| state.project.track(id))
                .and_then(|track| {
                    track
                        .gain_envelope()
                        .last()
                        .map(|point| point.beat)
                        .into_iter()
                        .chain(track.gain_lane().spans().last().map(|span| span.end))
                        .max()
                })
                .and_then(|beat| state.project.tempo_map().beat_to_frame(beat).ok())
        });
        let device_end = self
            .state
            .as_ref()
            .and_then(|state| self.automation_view.devices.display_end(&state.project));
        let horizon = self
            .retained_navigation_horizon(horizon)
            .max(gain_end.map_or(0, |frame| frame.saturating_add(1)))
            .max(device_end.map_or(0, |frame| frame.saturating_add(1)));
        // Only a current resize owner may extend the transient display extent.
        horizon.max(self.trim_display_end().unwrap_or(0))
    }

    fn visible_viewport(&self, horizon: u64) -> Viewport {
        let horizon = self.viewport_horizon(horizon);
        self.viewport
            .unwrap_or_else(|| Viewport::fit(horizon))
            .clamp(horizon)
    }

    pub(super) fn selected_track(&self) -> Option<TrackId> {
        self.selected_track
    }

    pub(super) fn selected_clip(&self) -> Option<ClipId> {
        self.selected_clip
    }

    pub(super) fn set_selected_clip(
        &mut self,
        track: TrackId,
        clip: ClipId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.focus_clip_details(window, cx) {
            return false;
        }
        self.advance_selection_intent();
        self.duplicate_intent.cancel_chain();
        self.retire_clip_detail_state_for(clip);
        self.detail_context = None;
        self.selected_track = Some(track);
        self.arrangement_focus.observe_active_track(Some(track));
        if !self.track_selection.contains(track)
            && let Some(state) = self.state.as_ref()
        {
            self.track_selection
                .select(&state.project, track, false, false);
            self.update_track_selection_status();
        }
        self.selected_clip = Some(clip);
        self.clip_selection.only(track, clip);
        self.focus.focus(window);
        true
    }

    /// Reset clip-local editor state when the clip editor's subject changes.
    pub(super) fn retire_clip_detail_state_for(&mut self, clip: ClipId) {
        if self.detail_clip_id() == Some(clip) {
            return;
        }
        self.piano_view_clip = None;
        self.zoom_fit.clear_clip();
        self.audio_event_editor.clear_fit_back();
        self.midi_creation = None;
        self.midi_note_gesture = None;
        self.midi_hit_cycle = None;
        self.midi_scene_cache.clear();
        self.midi_selection.clear();
        self.piano_beat_start = 0;
        if let Some(length) = self
            .state
            .as_ref()
            .and_then(|state| state.project.midi_clip(clip))
            .map(|midi| midi.source_length())
        {
            self.piano_beat_span = hex_project::BeatTime::from_raw(
                self.piano_beat_span.raw().min(length.raw().max(1)),
            );
        }
    }

    pub(super) fn applied_project_revision(&self) -> Option<u64> {
        self.applied_project_revision
    }

    pub(super) fn audio_label(&self) -> Option<&str> {
        self.audio.as_ref().map(|audio| audio.label.as_str())
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ArrangementPan {
    generation: u64,
    revision: u64,
    last_x: f32,
    last_y: f32,
    timeline_width: f32,
}

#[cfg(feature = "ui-test-support")]
pub(crate) fn sampler_source_fixture_view(
    window: gpui::AnyWindowHandle,
    track: hex_project::TrackId,
    cx: &gpui::App,
) -> Result<Option<crate::SamplerSourceTestView>, String> {
    window
        .read(cx, |view: gpui::Entity<EditorView>, cx| {
            let view = view.read(cx);
            let state = view.state.as_ref()?;
            let device = state.project.track(track)?.instrument()?;
            let reference = device.state().asset_references().first()?;
            let asset = state
                .project
                .assets()
                .find(|a| a.id().value() == reference.asset.value())?;
            Some(crate::SamplerSourceTestView {
                device: device.id(),
                asset: asset.id(),
                generation: view.project_generation,
                revision: state.revision,
                source_frames: asset.source_frames(),
                playback_frames: asset.playback_frames(),
                original_source: asset.original_source(),
                overviews: state.source_overviews.clone(),
            })
        })
        .map_err(|e| e.to_string())
}

#[cfg(feature = "ui-test-support")]
pub(crate) fn sampler_keyboard_fixture_status(
    window: gpui::AnyWindowHandle,
    cx: &gpui::App,
) -> Result<String, String> {
    window.read(cx, |view: gpui::Entity<EditorView>, cx| {
        let view = view.read(cx);
        format!("enabled={} suspended={} target={:?} request={} input={} cancelled={} desired={:?} current={} edit_pending={} error={:?} clip={:?} draw={} stroke={} note_count={:?} selected_notes={} worker_pending={} physical={} callback_refused={}",
            view.computer_keyboard.enabled, view.computer_keyboard.suspended,
            view.computer_keyboard.target,
            view.computer_keyboard.request_id,
            view.computer_keyboard.input.is_some(),
            view.computer_keyboard.input.as_ref().is_some_and(|i| i.is_cancelled()),
            view.computer_keyboard.desired_notes(), view.first_sampler_current(), view.edit_pending(), view.error, view.selected_clip,view.midi_draw_mode,view.midi_stroke.is_some(),view.selected_clip.and_then(|id|view.state.as_ref()?.project.midi_source(id).map(|s|s.notes().len())),view.midi_selection.len(),view.worker_requests_pending,view.computer_keyboard.physical_notes().count(),view.computer_keyboard.input.as_ref().is_some_and(|input|input.rejected()))
    }).map_err(|e| e.to_string())
}

#[cfg(feature = "ui-test-support")]
pub(crate) fn sampler_midi_fixture_ready(
    window: gpui::AnyWindowHandle,
    cx: &gpui::App,
) -> Result<bool, String> {
    window
        .read(cx, |view: gpui::Entity<EditorView>, cx| {
            let view = view.read(cx);
            view.first_sampler_current()
                && !view.edit_pending()
                && view.stopped_playback_restore.is_none()
        })
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod shortcut_tests {
    use super::{
        FitTimeline, KeyBinding, PanTimelineLeft, PanTimelineRight, SaveAsProject, SaveProject,
        SelectAllMidiNotes, ToggleMidiDrawMode, ZoomIn, ZoomOut, escape_binding,
    };

    #[test]
    fn timeline_shortcuts_parse() {
        let _zoom_in = KeyBinding::new("cmd-=", ZoomIn, Some("hex_editor"));
        let _zoom_out = KeyBinding::new("cmd--", ZoomOut, Some("hex_editor"));
        let _fit = KeyBinding::new("cmd-0", FitTimeline, Some("hex_editor"));
        let _pan_left = KeyBinding::new("alt-left", PanTimelineLeft, Some("hex_editor"));
        let _pan_right = KeyBinding::new("alt-right", PanTimelineRight, Some("hex_editor"));
        let _save = KeyBinding::new("cmd-s", SaveProject, Some("hex_editor"));
        let _save_as = KeyBinding::new("cmd-shift-s", SaveAsProject, Some("hex_editor"));
        let _escape = escape_binding();
        let _select_all_midi = KeyBinding::new("cmd-a", SelectAllMidiNotes, Some("hex_midi"));
        let _toggle_midi_draw = KeyBinding::new("b", ToggleMidiDrawMode, Some("hex_midi"));
    }
}
