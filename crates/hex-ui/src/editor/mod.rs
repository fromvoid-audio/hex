mod arrangement;
mod arrangement_audio_panel;
mod arrangement_device_drop;
#[cfg(all(test, feature = "ui-test-support"))]
mod arrangement_device_drop_ui_test;
#[cfg(all(test, feature = "ui-test-support"))]
mod arrangement_main_track_ui_test;
mod arrangement_seek;
mod audio_admission;
mod audio_event_editor;
mod browser;
mod browser_input;
#[cfg(all(test, feature = "ui-test-support"))]
mod clip_border_ui_test;
#[cfg(all(test, feature = "ui-test-support"))]
mod clip_clipboard_ui_test;
mod clip_content_cache;
#[cfg(all(test, feature = "ui-test-support"))]
mod clip_drag_ui_test;
mod clip_name;
mod clip_selection;
#[cfg(test)]
mod test_environment;

pub(crate) mod changelog;
mod clip_paint;
#[cfg(all(test, feature = "ui-test-support"))]
mod clip_paint_ui_test;
mod clip_view;
#[cfg(all(test, feature = "ui-test-support"))]
mod clip_view_ui_test;
mod close;
mod computer_keyboard;
mod context_menu;
mod controller;
mod controller_input;
mod device_gesture;
mod device_panel;
mod device_preset;
mod dialogs;
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
mod loop_rail;
#[cfg(all(test, feature = "ui-test-support"))]
mod loop_rail_ui_test;
mod loop_region;
mod menus;
mod midi;
mod midi_canvas;
mod midi_drag;
mod midi_draw;
mod midi_group_drag;
mod midi_scene;
mod midi_selection;
mod midi_source_controls;
mod midi_stroke;
mod midi_thumbnail;
mod mixer;
mod musical_audio_controls;
mod musical_time;
#[cfg(all(test, feature = "ui-test-support"))]
mod output_meter_ui_test;
mod overview;
mod play_intent;
mod project_directory;
mod project_recovery;
mod project_spectrum;
mod recent_projects;
mod recovery;
mod recovery_state;
mod refresh;
mod render;
mod return_tracks;
mod routing_controls;
mod save;
mod scrub;
mod seek;
mod selection;
mod session_audio_panel;
mod session_grid;
mod session_navigation;
#[cfg(all(test, feature = "ui-test-support"))]
mod session_navigation_ui_test;
mod session_route_view;
#[cfg(all(test, feature = "ui-test-support"))]
mod snap_nudge_ui_test;
mod tempo_input;
#[cfg(all(test, feature = "ui-test-support"))]
mod tempo_input_ui_test;
mod theme_settings;
mod time_gesture;
mod time_move_preview;
mod time_selection;
pub(super) mod time_selection_profile;
mod track_gain;
#[cfg(all(test, feature = "ui-test-support"))]
mod track_keyboard_ui_test;
mod track_layout;
#[cfg(all(test, feature = "ui-test-support"))]
mod track_layout_ui_test;
mod track_name;
#[cfg(all(test, feature = "ui-test-support"))]
mod track_selection_ui_test;
mod tracks;
mod transport;
mod trim;
mod tuning;
mod viewport;
mod waveform;
mod waveform_geometry;
mod waveform_stretch;

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
use std::{collections::VecDeque, rc::Rc, sync::Arc, time::Duration};
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
        error: String,
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
        ToggleBrowser,
        SearchBrowser,
        ToggleSnapGrid,
        ThemeSettingsEscape,
        BrowserPrevious,
        BrowserNext,
        BrowserActivate,
        BrowserPreviewSelected,
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
        OpenSettings,
        OpenTuning,
        CloseTuning,
        CommitTuningName,
        CancelTuningName,
        SaveProject,
        SaveAsProject,
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
        RenameClip,
        RenameTrack,
        CommitTrackName,
        CancelTrackName,
        CommitClipName,
        CancelClipName,
        CopyTimeSelection,
        CutTimeSelection,
        PasteTimeSelection,
        InsertTimeSelection,
        DuplicateClip,
        SplitClip,
        SetLoopFromClip,
        ToggleLoop,
        ZoomIn,
        ZoomOut,
        FitTimeline,
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
            cx.quit();
        }
    })
    .detach();
    Ok(())
}

// Text drafts own printable keys; modified commands retain their finalization paths.
const EDITOR_PRINTABLE_KEY_CONTEXT: &str = "hex_editor && !hex_text_editing";

fn bind_editor_keys(cx: &mut App) {
    changelog::bind_keys(cx);
    hex_widgets::bind_response_graph_keys(cx);
    device_panel::bind_operator_keys(cx);
    clip_name::bind_keys(cx);
    tuning::library::bind_keys(cx);
    track_name::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new("space", TogglePlay, Some(EDITOR_PRINTABLE_KEY_CONTEXT)),
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
        KeyBinding::new("cmd-f", SearchBrowser, None),
        KeyBinding::new("up", BrowserPrevious, Some("hex_browser")),
        KeyBinding::new("down", BrowserNext, Some("hex_browser")),
        KeyBinding::new("enter", BrowserActivate, Some("hex_browser")),
        KeyBinding::new("right", BrowserPreviewSelected, Some("hex_browser")),
        KeyBinding::new("left", BrowserCollapseFolder, Some("hex_browser")),
        KeyBinding::new("escape", BrowserEscape, Some("hex_browser")),
        KeyBinding::new("up", BrowserPrevious, Some("hex_browser_search")),
        KeyBinding::new("down", BrowserNext, Some("hex_browser_search")),
        KeyBinding::new("enter", BrowserActivate, Some("hex_browser_search")),
        KeyBinding::new("escape", BrowserEscape, Some("hex_browser_search")),
        gpui::KeyBinding::new(
            "backspace",
            browser_input::BrowserBackspace,
            Some("hex_browser_search"),
        ),
        gpui::KeyBinding::new(
            "delete",
            browser_input::BrowserDelete,
            Some("hex_browser_search"),
        ),
        gpui::KeyBinding::new(
            "left",
            browser_input::BrowserLeft,
            Some("hex_browser_search"),
        ),
        gpui::KeyBinding::new(
            "right",
            browser_input::BrowserRight,
            Some("hex_browser_search"),
        ),
        gpui::KeyBinding::new(
            "shift-left",
            browser_input::BrowserSelectLeft,
            Some("hex_browser_search"),
        ),
        gpui::KeyBinding::new(
            "shift-right",
            browser_input::BrowserSelectRight,
            Some("hex_browser_search"),
        ),
        gpui::KeyBinding::new(
            "cmd-a",
            browser_input::BrowserSelectAll,
            Some("hex_browser_search"),
        ),
        gpui::KeyBinding::new(
            "home",
            browser_input::BrowserHome,
            Some("hex_browser_search"),
        ),
        gpui::KeyBinding::new("end", browser_input::BrowserEnd, Some("hex_browser_search")),
        gpui::KeyBinding::new(
            "cmd-v",
            browser_input::BrowserPaste,
            Some("hex_browser_search"),
        ),
        gpui::KeyBinding::new(
            "cmd-c",
            browser_input::BrowserCopy,
            Some("hex_browser_search"),
        ),
        gpui::KeyBinding::new(
            "cmd-x",
            browser_input::BrowserCut,
            Some("hex_browser_search"),
        ),
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
        KeyBinding::new("left", MoveLeft, Some("hex_editor")),
        KeyBinding::new("right", MoveRight, Some("hex_editor")),
        KeyBinding::new("shift-left", TrimLeft, Some("hex_editor")),
        KeyBinding::new("shift-right", TrimRight, Some("hex_editor")),
        KeyBinding::new("backspace", DeleteClip, Some("hex_editor")),
        KeyBinding::new("delete", DeleteClip, Some("hex_editor")),
        KeyBinding::new("cmd-c", CopyTimeSelection, Some("hex_editor")),
        KeyBinding::new("cmd-shift-c", CopyTimeSelection, Some("hex_editor")),
        KeyBinding::new("cmd-x", CutTimeSelection, Some("hex_editor")),
        KeyBinding::new("cmd-v", PasteTimeSelection, Some("hex_editor")),
        KeyBinding::new("cmd-shift-v", InsertTimeSelection, Some("hex_editor")),
        KeyBinding::new("cmd-d", DuplicateClip, Some("hex_editor")),
        KeyBinding::new("cmd-alt-up", MoveSceneUp, Some("hex_editor")),
        KeyBinding::new("cmd-alt-down", MoveSceneDown, Some("hex_editor")),
        KeyBinding::new("cmd-e", SplitClip, Some("hex_editor")),
        KeyBinding::new("cmd-l", SetLoopFromClip, Some("hex_editor")),
        KeyBinding::new("l", ToggleLoop, Some(EDITOR_PRINTABLE_KEY_CONTEXT)),
        KeyBinding::new("cmd-=", ZoomIn, Some("hex_editor")),
        KeyBinding::new("cmd-+", ZoomIn, Some("hex_editor")),
        KeyBinding::new("cmd--", ZoomOut, Some("hex_editor")),
        KeyBinding::new("cmd-0", FitTimeline, Some("hex_editor")),
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
    cx.open_window(editor_window_options(cx), move |window, cx| {
        let view = cx.new(|cx| EditorView::new_with_backend(window, cx, backend));
        install_close_guard(window, cx, view.downgrade());
        view
    })
    .map_err(|error| format!("could not open hex: {error}"))?;
    Ok(())
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
    audio: Option<crate::EditorAudio>,
    audio_events: std::sync::mpsc::Receiver<crate::EditorAudioEvent>,
    source_reservation: Option<hex_session::SourcePlanReservation>,
    restart_request_id: u64,
    pending_restart_request: Option<(u64, u64)>,
    pending_recovery_generation: Option<u64>,
    pending_audio_result: Option<PendingEditorAudioResult>,
    pending_publication_observation: Option<PendingEditorPublicationObservation>,
    document_audio_mode: hex_session::DocumentAudioMode,
    recovery: RecoveryState,
    stopped_playback_restore: Option<audio_admission::StoppedPlaybackRestore>,
    offline_cursor: u64,
    pub(in crate::editor) state: Option<Arc<ViewState>>,
    selected_track: Option<TrackId>,
    track_selection: selection::TrackSelection,
    gain_lane: gain_lane::GainLane,
    sample_bpm_control: Option<musical_audio_controls::sample_bpm::Binding>,
    project_tempo_control: Option<tempo_input::Binding>,
    track_gain_controls: track_gain::Controls,
    gain_recorder: gain_recording::Recorder,
    routing_open: bool,
    routing_page: usize,
    pub(in crate::editor) selected_clip: Option<ClipId>,
    clip_selection: clip_selection::ClipSelection,
    clip_name_edit: Option<clip_name::NameEdit>,
    track_name: track_name::TrackNameState,
    pub(in crate::editor) mode: EditorMode,
    pub(in crate::editor) selected_scene: Option<SceneId>,
    pub(in crate::editor) selected_cell: Option<(TrackId, SceneId)>,
    pub(in crate::editor) session_copy_source: Option<ClipId>,
    pub(in crate::editor) session_track_start: usize,
    pub(in crate::editor) session_visible_columns: usize,
    pub(in crate::editor) session_quantization: hex_engine::LaunchQuantization,
    pub(in crate::editor) midi_selection: midi_selection::MidiSelection,
    pub(in crate::editor) midi_note_gesture: Option<midi_drag::NoteGesture>,
    pub(in crate::editor) midi_group_gesture: Option<midi_group_drag::GroupGesture>,
    pub(in crate::editor) midi_stroke: Option<midi_stroke::MidiStroke>,
    pub(in crate::editor) midi_draw_mode: bool,
    pub(in crate::editor) midi_draw_pitch_locked: bool,
    pub(in crate::editor) midi_marker: Option<u64>,
    pub(in crate::editor) midi_marquee: Option<([f32; 2], [f32; 2], bool)>,
    pub(in crate::editor) midi_scene_cache: midi_scene::MidiSceneCache,
    pub(in crate::editor) midi_hit_cycle: Option<midi_scene::HitCycle>,
    pub(in crate::editor) piano_beat_start: hex_project::BeatTime,
    pub(in crate::editor) piano_beat_span: hex_project::BeatTime,
    pub(in crate::editor) piano_pitch_low: u8,
    pub(in crate::editor) midi_snap_grid: midi_drag::MidiGrid,
    pending_requests: VecDeque<Request>,
    device_gesture: hex_session::device_gesture::GestureClient,
    device_gesture_barriers: VecDeque<Request>,
    device_panel: device_panel::DevicePanelState,
    project_spectrum: project_spectrum::ProjectSpectrumState,
    worker_requests_pending: usize,
    audio_batch: Option<Arc<hex_session::import_batch_decode::AudioBatchControl>>,
    audio_batch_progress: Option<hex_session::import_batch_decode::AudioBatchProgress>,
    pending_playback: Option<PendingPlayback>,
    pending_resident_publication: Option<PendingResidentPublication>,
    pending_audio_commands: VecDeque<PendingPlaybackAction>,
    pending_seek_ticket: Option<u64>,
    pending_metronome_ticket: Option<u64>,
    pending_session_ticket: Option<(u64, u64)>,
    viewport: Option<Viewport>,
    playback_candidates: VecDeque<PlaybackCandidate>,
    audible_playback: Option<PlaybackCandidate>,
    audio_admission: AudioAdmissionStatus,
    session_routes: session_route_view::RouteView,
    prepared_seek: Option<arrangement_seek::PendingSeek>,
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
    pending_new_mode: Option<hex_session::DocumentAudioMode>,
    duplicate_intent: DuplicateIntent,
    pending_scene_duplicate: Option<PendingSceneDuplicate>,
    close_prompt_open: bool,
    export_pending: bool,
    export_control: Option<hex_session::service::ExportControl>,
    export_window: Option<gpui::AnyWindowHandle>,
    export_dialog_open: bool,
    saving_project: bool,
    save_dialog_open: bool,
    preset_dialog_open: bool,
    status: String,
    pub(in crate::editor) error: Option<display_error::DisplayError>,
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
    focused_return: Option<hex_project::ReturnId>,
    send_menu_track: Option<TrackId>,
    pub(in crate::editor) session_scene_scroll: gpui::UniformListScrollHandle,
    redraw_gate: RedrawGate,
    output_meter_frame: Option<refresh::AudioFrame>,
    pub(in crate::editor) project_generation: u64,
    clip_view_open: bool,
    device_panel_active: bool,
    clip_detail_body_height: f32,
    loop_rail_gesture: Option<loop_rail::LoopRailGesture>,
    loop_rail_cursor: gpui::CursorStyle,
    clip_content_cache: clip_content_cache::TrackCache,
    drag_lanes: std::collections::HashMap<TrackId, drag::LaneBounds>,
    drag_lane_epochs: std::collections::HashMap<TrackId, u64>,
    arrangement_bounds: Option<drag::LaneBounds>,
    arrangement_bounds_epoch: u64,
    main_track_lane_bounds: Option<drag::LaneBounds>,
    main_track_lane_epoch: u64,
    drag_layout_generation: u64,
    arrangement_pan: Option<ArrangementPan>,
    scrub: scrub::ScrubState,
    zoom_wheel_accum: f32,
    overview_cache: std::cell::RefCell<Option<Rc<overview::OverviewCache>>>,
    audio_point_page: usize,
    pub(in crate::editor) waveform_geometry_cache:
        Rc<std::cell::RefCell<waveform_geometry::WaveformGeometryCache>>,
    active_clip_drag: Option<drag::ClipDrag>,
    active_browser_audio_drag: Option<drag::BrowserAudioDrag>,
    arrangement_device_preview: Option<arrangement_device_drop::ArrangementDevicePreview>,
    active_native_device_drag: Option<drag::NativeDeviceDrag>,
    active_trim_drag: Option<trim::TrimDrag>,
    pending_trim: Option<trim::PendingTrim>,
    fade_key_held: bool,
    active_fade_drag: Option<fade_handles::FadeDrag>,
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
    #[cfg(test)]
    test_environment: Arc<test_environment::Environment>,
    browser: browser::BrowserState,
    recent_projects: RecentProjectsState,
    project_directory: project_directory::State,
    changelog: changelog::State,
    theme_settings: theme_settings::ThemeSettingsState,
    tuning: tuning::TuningState,
    context_menu: Option<crate::context_menu::PopupMenu<context_menu::EditorContextAction>>,
    edit_cursor: Option<edit_cursor::EditCursor>,
    clip_paint_gesture: Option<clip_paint::ClipPaintGesture>,
    time_selection: Option<time_selection::TimeSelection>,
    time_selection_project_scope: bool,
    time_selection_gesture: Option<time_selection::TimeSelectionGesture>,
    time_range_move_gesture: Option<time_selection::TimeRangeMoveGesture>,
    pending_time_range_move: Option<time_selection::PendingTimeRangeMove>,
    suppress_cursor_click: bool,
}

impl EditorView {
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
            test_environment::Inputs {
                directory: Some(directory.join("samples")),
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
        #[cfg(test)] test_paths: test_environment::Inputs,
    ) -> Self {
        #[cfg(test)]
        let test_environment = test_environment::Environment::new()
            .expect("could not create private editor test environment; no ambient fallback");
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
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                if this
                    .update(cx, |view, cx| {
                        if view.tick(cx) {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let keyboard_window_id = window.window_handle().window_id();
        let keyboard_entity = cx.entity().downgrade();
        let computer_keyboard_keys = cx.intercept_keystrokes(move |event, window, cx| {
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
            if !view.focus.is_focused(window) {
                view.cancel_fade_gesture(cx);
                view.cancel_project_spectrum_resize(cx);
                if view.cancel_trim_gesture(cx) {
                    cx.stop_active_drag(window);
                }
                view.cancel_track_resize(cx);
            }
            view.cancel_computer_keyboard();
            view.computer_keyboard.suspended = true;
            if view.cancel_scrub() | view.cancel_clip_paint() {
                cx.notify();
            }
        });
        let audio_event_focus = cx.focus_handle();
        let audio_event_blur = cx.on_blur(&audio_event_focus, window, |view, _, cx| {
            view.audio_event_editor.gesture = None;
            cx.notify();
        });
        let midi_focus = cx.focus_handle();
        let focus_blur = cx.on_blur(&midi_focus, window, |view, _, cx| {
            view.cancel_computer_keyboard();
            view.computer_keyboard.suspended = true;
            let midi_cancelled = view.cancel_midi_note_gesture();
            if midi_cancelled {
                cx.notify();
            }
        });
        let window_activation = cx.observe_window_activation(window, |view, window, cx| {
            if !window.is_window_active() {
                view.cancel_tuning_audition();
                view.cancel_fade_gesture(cx);
                view.cancel_project_spectrum_resize(cx);
                if view.cancel_trim_gesture(cx) {
                    cx.stop_active_drag(window);
                }
                view.cancel_track_resize(cx);
                view.cancel_computer_keyboard();
                view.computer_keyboard.suspended = true;
                view.audio_event_editor.gesture = None;
                let midi_cancelled = view.cancel_midi_note_gesture();
                let scrub_cancelled = view.cancel_scrub();
                let paint_cancelled = view.cancel_clip_paint();
                if let Some(audio) = view.audio.as_mut() {
                    let _ = audio.control.audition_cancel();
                }
                device_panel::clear_eq8_audition_gesture_flags(view);
                if midi_cancelled || scrub_cancelled || paint_cancelled {
                    cx.notify();
                }
            }
        });
        let window_bounds = cx.observe_window_bounds(window, |view, window, cx| {
            view.cancel_fade_gesture(cx);
            view.cancel_project_spectrum_resize(cx);
            if view.cancel_trim_gesture(cx) {
                cx.stop_active_drag(window);
            }
            view.cancel_track_resize(cx);
            view.audio_event_editor.gesture = None;
            if view.cancel_midi_note_gesture() | view.cancel_scrub() | view.cancel_clip_paint() {
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
        let theme_settings =
            theme_settings::ThemeSettingsState::new(theme_preferences_path, window, cx);
        let mut view = Self {
            service,
            audio,
            audio_events: std::sync::mpsc::channel().1,
            source_reservation: None,
            restart_request_id: 0,
            pending_restart_request: None,
            pending_recovery_generation: None,
            pending_audio_result: None,
            pending_publication_observation: None,
            document_audio_mode: hex_session::DocumentAudioMode::Resident,
            state: None,
            recovery: RecoveryState::default(),
            stopped_playback_restore: None,
            offline_cursor: 0,
            selected_track: None,
            track_selection: selection::TrackSelection::default(),
            gain_lane: gain_lane::GainLane::default(),
            sample_bpm_control: None,
            project_tempo_control: None,
            track_gain_controls: track_gain::Controls::default(),
            gain_recorder: gain_recording::Recorder::default(),
            routing_open: false,
            routing_page: 0,
            selected_clip: None,
            clip_selection: clip_selection::ClipSelection::default(),
            clip_name_edit: None,
            track_name: track_name::TrackNameState::default(),
            mode: EditorMode::Arrangement,
            selected_scene: None,
            selected_cell: None,
            session_copy_source: None,
            session_track_start: 0,
            session_visible_columns: 1,
            session_quantization: hex_engine::LaunchQuantization::Immediate,
            midi_selection: midi_selection::MidiSelection::default(),
            midi_note_gesture: None,
            midi_group_gesture: None,
            midi_stroke: None,
            midi_draw_mode: false,
            midi_draw_pitch_locked: true,
            midi_marker: None,
            midi_marquee: None,
            midi_scene_cache: midi_scene::MidiSceneCache::default(),
            midi_hit_cycle: None,
            piano_beat_start: hex_project::BeatTime::ZERO,
            piano_beat_span: hex_project::BeatTime::from_quarters(4)
                .unwrap_or(hex_project::BeatTime::ONE),
            piano_pitch_low: 48,
            midi_snap_grid: midi_drag::MidiGrid::default(),
            pending_requests: VecDeque::new(),
            device_gesture: hex_session::device_gesture::GestureClient::default(),
            device_gesture_barriers: VecDeque::new(),
            device_panel: device_panel::DevicePanelState::default(),
            project_spectrum: Default::default(),
            worker_requests_pending: 0,
            audio_batch: None,
            audio_batch_progress: None,
            pending_playback: None,
            pending_resident_publication: None,
            pending_audio_commands: VecDeque::new(),
            pending_seek_ticket: None,
            pending_metronome_ticket: None,
            pending_session_ticket: None,
            viewport: None,
            playback_candidates: VecDeque::new(),
            audible_playback: None,
            audio_admission: AudioAdmissionStatus::default(),
            session_routes: session_route_view::RouteView::default(),
            prepared_seek: None,
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
            pending_new_mode: None,
            duplicate_intent: DuplicateIntent::default(),
            pending_scene_duplicate: None,
            close_prompt_open: false,
            export_pending: false,
            export_control: None,
            export_window: None,
            export_dialog_open: false,
            saving_project: false,
            save_dialog_open: false,
            preset_dialog_open: false,
            status,
            error: None,
            computer_keyboard: computer_keyboard::ComputerKeyboard::default(),
            controller_input: controller_input::ControllerInput::default(),
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
            focused_return: None,
            send_menu_track: None,
            session_scene_scroll: gpui::UniformListScrollHandle::new(),
            redraw_gate: RedrawGate::default(),
            output_meter_frame: None,
            project_generation: 0,
            clip_view_open: true,
            device_panel_active: false,
            clip_detail_body_height: clip_view::CLIP_DETAIL_BODY_HEIGHT,
            loop_rail_gesture: None,
            loop_rail_cursor: gpui::CursorStyle::Crosshair,
            clip_content_cache: clip_content_cache::TrackCache::default(),
            drag_lanes: std::collections::HashMap::new(),
            drag_lane_epochs: std::collections::HashMap::new(),
            arrangement_bounds: None,
            arrangement_bounds_epoch: 0,
            main_track_lane_bounds: None,
            main_track_lane_epoch: 0,
            drag_layout_generation: 0,
            arrangement_pan: None,
            scrub: scrub::ScrubState::default(),
            zoom_wheel_accum: 0.0,
            overview_cache: std::cell::RefCell::new(None),
            audio_point_page: 0,
            waveform_geometry_cache: Rc::new(std::cell::RefCell::new(
                waveform_geometry::WaveformGeometryCache::default(),
            )),
            active_clip_drag: None,
            active_browser_audio_drag: None,
            active_native_device_drag: None,
            arrangement_device_preview: None,
            active_trim_drag: None,
            pending_trim: None,
            fade_key_held: false,
            active_fade_drag: None,
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
            #[cfg(test)]
            test_environment,
            browser,
            recent_projects,
            project_directory,
            changelog: changelog::State::new(cx),
            theme_settings,
            tuning: tuning::TuningState::new(cx),
            context_menu: None,
            edit_cursor: None,
            clip_paint_gesture: None,
            time_selection: None,
            time_selection_project_scope: false,
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
        view.browser_scan_initial(cx);
        view.start_recent_projects_load(cx);
        view.start_project_directory_load(cx);
        view.start_theme_preferences_load(cx);
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
        let header_focused = self.track_header_focused(window);
        self.duplicate_intent.cancel_chain();
        self.midi_note_gesture = None;
        self.midi_hit_cycle = None;
        self.focus_track_details(window);
        self.selected_track = Some(track);
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
        if self.recovery.generation().is_some() {
            self.error = Some("audio recovery pending · playback is not ready".into());
            return;
        }
        if self.opening_project || self.close_prompt_open {
            self.error = Some("project change pending · playback is paused".into());
            return;
        }
        let device_playing = self.audio.as_ref().is_some_and(|audio| {
            audio
                .control
                .status()
                .is_some_and(|status| status.playing.load(std::sync::atomic::Ordering::Acquire))
        });
        let start_frame = self.playback_start_frame();
        let command = match self.play_toggle_action(device_playing, start_frame) {
            arrangement_seek::ToggleAction::Pause => PlaybackCommand::Pause,
            arrangement_seek::ToggleAction::PlayFrom(frame) => PlaybackCommand::PlayFrom(frame),
            arrangement_seek::ToggleAction::PreparedPlayFrom(frame) => {
                self.request_prepared_seek(frame, true);
                return;
            }
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
        if let Some(selection) = self.normalized_time_selection() {
            return selection.start();
        }
        self.edit_cursor
            .filter(|cursor| cursor.generation == self.project_generation)
            .map_or(0, |cursor| cursor.frame)
    }

    pub(super) fn toggle_solo(&mut self, track: TrackId, exclusive: bool) {
        self.enqueue(Request::Edit(Command::ToggleSolo { track, exclusive }));
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
        let choice = self
            .state
            .as_ref()
            .and_then(|state| selection::neighbor(&state.project, self.selected_track, forward));
        if let Some((index, track, clip)) = choice {
            self.select_track(track, window);
            if let Some(clip) = clip
                && self.focus_clip_details(window, cx)
            {
                self.selected_clip = Some(clip);
            }
            self.track_scroll
                .scroll_to_item(index, gpui::ScrollStrategy::Top);
        }
    }

    pub(super) fn toggle_mute(&mut self, track: TrackId) {
        self.enqueue(Request::Edit(Command::ToggleMute(track)));
    }

    pub(super) fn gain_by(&mut self, track: TrackId, amount: f32) {
        if self
            .state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .is_some_and(|track| {
                !track.gain_envelope().is_empty() || !track.gain_lane().spans().is_empty()
            })
        {
            self.error = Some(
                "automation owns gain · edit the gain lane or clear it for static fallback".into(),
            );
            return;
        }
        self.enqueue(Request::Edit(Command::GainBy { track, amount }));
    }

    pub(super) fn add_track(&mut self) {
        let count = self
            .state
            .as_ref()
            .map_or(0, |state| state.project.tracks().len());
        self.enqueue(Request::Edit(Command::AddTrack {
            name: format!("audio {}", count + 1),
        }));
    }

    pub(super) fn remove_track(&mut self, cx: &mut Context<Self>) {
        if !device_panel::finalize_controls(self, cx) {
            return;
        }
        if let (Some(track), Some(state)) = (self.selected_track, self.state.as_ref()) {
            if state.project.track(track).is_some() {
                self.duplicate_intent.cancel_chain();
                self.enqueue_device_barrier(Request::Edit(Command::RemoveTrack(track)));
                self.selected_track = None;
                self.selected_clip = None;
            }
        }
    }

    pub(super) fn state(&self) -> Option<&Arc<ViewState>> {
        self.state.as_ref()
    }

    pub(super) fn audio_status(&self) -> Option<&Status> {
        self.audio.as_ref().and_then(|audio| audio.status())
    }

    fn visible_viewport(&self, horizon: u64) -> Viewport {
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
        let horizon = horizon.max(gain_end.map_or(0, |frame| frame.saturating_add(1)));
        // Only a current resize owner may extend the transient display extent.
        let horizon = horizon.max(self.trim_display_end().unwrap_or(0));
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
        self.duplicate_intent.cancel_chain();
        if self.selected_clip != Some(clip) {
            self.midi_note_gesture = None;
            self.midi_hit_cycle = None;
            self.midi_scene_cache.clear();
            self.midi_selection.clear();
            self.piano_beat_start = hex_project::BeatTime::ZERO;
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
        self.selected_track = Some(track);
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
