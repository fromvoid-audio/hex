mod arrangement;
mod arrangement_audio_panel;
mod bootstrap;
mod browser;
mod browser_input;
#[cfg(all(test, feature = "ui-test-support"))]
mod clip_drag_ui_test;
mod clip_view;
#[cfg(all(test, feature = "ui-test-support"))]
mod clip_view_ui_test;
mod close;
mod context_menu;
mod controller;
mod device_gesture;
mod dialogs;
mod drag;
mod duplicate;
mod edit_cursor;
#[cfg(all(test, feature = "ui-test-support"))]
mod edit_cursor_ui_test;
mod export;
mod fades;
mod header;
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
mod musical_time;
mod overview;
mod play_intent;
mod recent_projects;
mod recovery;
mod recovery_state;
mod refresh;
mod render;
mod save;
mod seek;
mod selection;
mod session_audio_panel;
mod session_grid;
mod session_navigation;
#[cfg(all(test, feature = "ui-test-support"))]
mod session_navigation_ui_test;
mod session_route_view;
mod time_gesture;
mod time_move_preview;
mod time_selection;
mod tracks;
mod transport;
mod trim;
mod viewport;
mod waveform;
mod waveform_geometry;

use crate::theme;
use duplicate::DuplicateIntent;
use gpui::{
    App, AppContext, Bounds, Context, FocusHandle, KeyBinding, Window, WindowBounds, WindowOptions,
    actions, px, size,
};
use hex_audio::{PreparedOutput, prepared::Status};
use hex_engine::playback::{Playback, PlaybackAction, PlaybackCommand};
use hex_project::{ClipId, Command, SceneId, TrackId};
use hex_session::service::{Request, Service, ViewState};
use recent_projects::RecentProjectsState;
use recovery_state::RecoveryState;
use refresh::RedrawGate;
use std::{collections::VecDeque, rc::Rc, sync::Arc, time::Duration};
use viewport::Viewport;

const MAX_RESIDENT_FRAMES: u64 = 8_000_000;
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
    device_preview_identity: Option<hex_session::service::DevicePreviewIdentity>,
}

struct PlaybackCandidate {
    audio_revision: u64,
    project_generation: u64,
    state: Arc<ViewState>,
    device_preview_identity: Option<hex_session::service::DevicePreviewIdentity>,
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
        BrowserPrevious,
        BrowserNext,
        BrowserActivate,
        BrowserEscape,
        ToggleSolo,
        AdditiveSolo,
        CancelDragOrStop,
        PreviousTrack,
        NextTrack,
        LaunchSessionSelection,
        Stop,
        ImportAudio,
        OpenProject,
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
    cx.on_window_closed(|cx| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
    Ok(())
}

fn bind_editor_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("space", TogglePlay, Some("hex_editor")),
        KeyBinding::new("s", ToggleSolo, Some("hex_editor")),
        KeyBinding::new("shift-s", AdditiveSolo, Some("hex_editor")),
        KeyBinding::new("up", PreviousTrack, Some("hex_editor")),
        KeyBinding::new("down", NextTrack, Some("hex_editor")),
        KeyBinding::new("enter", LaunchSessionSelection, Some("hex_editor")),
        escape_binding(),
        KeyBinding::new("cmd-i", ImportAudio, Some("hex_editor")),
        KeyBinding::new("cmd-alt-b", ToggleBrowser, None),
        KeyBinding::new("up", BrowserPrevious, Some("hex_browser")),
        KeyBinding::new("down", BrowserNext, Some("hex_browser")),
        KeyBinding::new("enter", BrowserActivate, Some("hex_browser")),
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
        KeyBinding::new("cmd-s", SaveProject, Some("hex_editor")),
        KeyBinding::new("cmd-shift-s", SaveAsProject, Some("hex_editor")),
        KeyBinding::new("cmd-shift-e", ExportProject, Some("hex_editor")),
        KeyBinding::new("cmd-z", Undo, Some("hex_editor")),
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
        KeyBinding::new("l", ToggleLoop, Some("hex_editor")),
        KeyBinding::new("cmd-=", ZoomIn, Some("hex_editor")),
        KeyBinding::new("cmd-+", ZoomIn, Some("hex_editor")),
        KeyBinding::new("cmd--", ZoomOut, Some("hex_editor")),
        KeyBinding::new("cmd-0", FitTimeline, Some("hex_editor")),
        KeyBinding::new("alt-left", PanTimelineLeft, Some("hex_editor")),
        KeyBinding::new("alt-right", PanTimelineRight, Some("hex_editor")),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-a", SelectAllMidiNotes, Some("hex_midi")),
        KeyBinding::new("b", ToggleMidiDrawMode, Some("hex_midi")),
        KeyBinding::new("shift-up", MidiOctaveUp, Some("hex_midi")),
        KeyBinding::new("shift-down", MidiOctaveDown, Some("hex_midi")),
    ]);
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

pub(super) fn open(cx: &mut App, output: Result<PreparedOutput, String>) -> Result<(), String> {
    configure_editor_app(cx)?;
    let sample_rate = output.as_ref().map_or(48_000, |audio| audio.sample_rate);
    let service = Service::new(sample_rate, 32, MAX_RESIDENT_FRAMES)
        .map_err(|error| error.to_string().to_lowercase())?;
    cx.open_window(editor_window_options(cx), move |window, cx| {
        let view = cx.new(|cx| EditorView::new(window, cx, service, output));
        install_close_guard(window, cx, view.downgrade());
        view
    })
    .map_err(|error| format!("could not open hex: {error}"))?;
    Ok(())
}

pub(super) fn open_pending(
    cx: &mut App,
    request: Result<hex_audio::RecoveryOpen, String>,
) -> Result<(), String> {
    configure_editor_app(cx)?;
    bootstrap::open(cx, request)
}

pub(super) struct EditorView {
    service: Service,
    audio: Option<PreparedOutput>,
    recovery_open: Option<hex_audio::RecoveryOpen>,
    recovery: RecoveryState,
    offline_cursor: u64,
    pub(in crate::editor) state: Option<Arc<ViewState>>,
    selected_track: Option<TrackId>,
    pub(in crate::editor) selected_clip: Option<ClipId>,
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
    worker_requests_pending: usize,
    pending_playback: Option<PendingPlayback>,
    pending_audio_commands: VecDeque<PendingPlaybackAction>,
    pending_seek_ticket: Option<u64>,
    pending_session_ticket: Option<(u64, u64)>,
    viewport: Option<Viewport>,
    applied_revisions: VecDeque<(u64, u64)>,
    playback_candidates: VecDeque<PlaybackCandidate>,
    audible_playback: Option<PlaybackCandidate>,
    session_routes: session_route_view::RouteView,
    applied_project_revision: Option<u64>,
    observed_rejected_action: u64,
    observed_rejected_revision: u64,
    observed_session_capability: Option<u32>,
    session_capability_message: Option<String>,
    play_intent: play_intent::Tracker,
    opening_project: bool,
    pending_open_path: Option<std::path::PathBuf>,
    duplicate_intent: DuplicateIntent,
    pending_scene_duplicate: Option<PendingSceneDuplicate>,
    close_prompt_open: bool,
    export_pending: bool,
    export_dialog_open: bool,
    saving_project: bool,
    save_dialog_open: bool,
    status: String,
    pub(in crate::editor) error: Option<String>,
    focus: FocusHandle,
    pub(in crate::editor) midi_focus: FocusHandle,
    _midi_focus_lifecycle: [gpui::Subscription; 3],
    track_scroll: gpui::UniformListScrollHandle,
    pub(in crate::editor) session_scene_scroll: gpui::UniformListScrollHandle,
    redraw_gate: RedrawGate,
    pub(in crate::editor) project_generation: u64,
    clip_view_open: bool,
    drag_lanes: std::collections::HashMap<TrackId, drag::LaneBounds>,
    drag_lane_epochs: std::collections::HashMap<TrackId, u64>,
    arrangement_bounds: Option<drag::LaneBounds>,
    arrangement_bounds_epoch: u64,
    drag_layout_generation: u64,
    arrangement_pan: Option<ArrangementPan>,
    zoom_wheel_accum: f32,
    overview_cache: std::cell::RefCell<Option<Rc<overview::OverviewCache>>>,
    pub(in crate::editor) waveform_geometry_cache:
        Rc<std::cell::RefCell<waveform_geometry::WaveformGeometryCache>>,
    active_clip_drag: Option<drag::ClipDrag>,
    active_browser_audio_drag: Option<drag::BrowserAudioDrag>,
    active_trim_drag: Option<trim::TrimDrag>,
    active_midi_trim_drag: Option<trim::MidiTrimDrag>,
    pointer_down: Option<drag::PointerDown>,
    #[cfg(test)]
    drag_move_event_count: usize,
    #[cfg(test)]
    last_enqueued_edit: Option<Command>,
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
    browser: browser::BrowserState,
    recent_projects: RecentProjectsState,
    context_menu: Option<crate::context_menu::PopupMenu<context_menu::InsertTrackAction>>,
    edit_cursor: Option<edit_cursor::EditCursor>,
    time_selection: Option<time_selection::TimeSelection>,
    time_selection_gesture: Option<time_selection::TimeSelectionGesture>,
    time_range_move_gesture: Option<time_selection::TimeRangeMoveGesture>,
    pending_time_range_move: Option<time_selection::PendingTimeRangeMove>,
    suppress_cursor_click: bool,
}

impl EditorView {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        service: Service,
        output: Result<PreparedOutput, String>,
    ) -> Self {
        Self::new_with_browser_preferences_path(
            window,
            cx,
            service,
            output,
            browser::default_preferences_path(),
            recent_projects::default_preferences_path(),
        )
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    fn new_for_test(
        window: &mut Window,
        cx: &mut Context<Self>,
        service: Service,
        output: Result<PreparedOutput, String>,
        preferences_path: std::path::PathBuf,
        recent_projects_path: std::path::PathBuf,
    ) -> Self {
        Self::new_with_browser_preferences_path(
            window,
            cx,
            service,
            output,
            preferences_path,
            recent_projects_path,
        )
    }

    fn new_with_browser_preferences_path(
        window: &mut Window,
        cx: &mut Context<Self>,
        service: Service,
        output: Result<PreparedOutput, String>,
        preferences_path: std::path::PathBuf,
        recent_projects_path: std::path::PathBuf,
    ) -> Self {
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
        let focus = cx.focus_handle();
        let midi_focus = cx.focus_handle();
        let focus_blur = cx.on_blur(&midi_focus, window, |view, _, cx| {
            if view.cancel_midi_note_gesture() {
                cx.notify();
            }
        });
        let window_activation = cx.observe_window_activation(window, |view, window, cx| {
            if !window.is_window_active() && view.cancel_midi_note_gesture() {
                cx.notify();
            }
        });
        let window_bounds = cx.observe_window_bounds(window, |view, _, cx| {
            if view.cancel_midi_note_gesture() {
                cx.notify();
            }
        });
        focus.focus(window);
        let browser = browser::BrowserState::new(cx, preferences_path);
        let recent_projects = RecentProjectsState::new(recent_projects_path);
        let mut view = Self {
            service,
            audio,
            state: None,
            recovery_open: None,
            recovery: RecoveryState::default(),
            offline_cursor: 0,
            selected_track: None,
            selected_clip: None,
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
            worker_requests_pending: 0,
            pending_playback: None,
            pending_audio_commands: VecDeque::new(),
            pending_seek_ticket: None,
            pending_session_ticket: None,
            viewport: None,
            applied_revisions: VecDeque::new(),
            playback_candidates: VecDeque::new(),
            audible_playback: None,
            session_routes: session_route_view::RouteView::default(),
            applied_project_revision: None,
            observed_rejected_action: 0,
            observed_rejected_revision: 0,
            observed_session_capability: None,
            session_capability_message: None,
            play_intent: play_intent::Tracker::default(),
            opening_project: false,
            pending_open_path: None,
            duplicate_intent: DuplicateIntent::default(),
            pending_scene_duplicate: None,
            close_prompt_open: false,
            export_pending: false,
            export_dialog_open: false,
            saving_project: false,
            save_dialog_open: false,
            status,
            error: None,
            focus,
            midi_focus,
            _midi_focus_lifecycle: [focus_blur, window_activation, window_bounds],
            track_scroll: gpui::UniformListScrollHandle::new(),
            session_scene_scroll: gpui::UniformListScrollHandle::new(),
            redraw_gate: RedrawGate::default(),
            project_generation: 0,
            clip_view_open: true,
            drag_lanes: std::collections::HashMap::new(),
            drag_lane_epochs: std::collections::HashMap::new(),
            arrangement_bounds: None,
            arrangement_bounds_epoch: 0,
            drag_layout_generation: 0,
            arrangement_pan: None,
            zoom_wheel_accum: 0.0,
            overview_cache: std::cell::RefCell::new(None),
            waveform_geometry_cache: Rc::new(std::cell::RefCell::new(
                waveform_geometry::WaveformGeometryCache::default(),
            )),
            active_clip_drag: None,
            active_browser_audio_drag: None,
            active_trim_drag: None,
            active_midi_trim_drag: None,
            pointer_down: None,
            #[cfg(test)]
            drag_move_event_count: 0,
            #[cfg(test)]
            last_enqueued_edit: None,
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
            browser,
            recent_projects,
            context_menu: None,
            edit_cursor: None,
            time_selection: None,
            time_selection_gesture: None,
            time_range_move_gesture: None,
            pending_time_range_move: None,
            suppress_cursor_click: false,
        };
        view.browser_scan_initial(cx);
        view.start_recent_projects_load(cx);
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

    pub(super) fn select_track(&mut self, track: TrackId) {
        self.duplicate_intent.cancel_chain();
        self.midi_note_gesture = None;
        self.midi_hit_cycle = None;
        self.selected_track = Some(track);
        self.selected_clip = None;
        self.midi_selection.clear();
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
                .status
                .playing
                .load(std::sync::atomic::Ordering::Acquire)
        });
        let playing = self.play_intent.desired_playing(device_playing);
        let command = if playing {
            PlaybackCommand::Pause
        } else {
            PlaybackCommand::PlayFrom(self.playback_start_frame())
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

    pub(super) fn select_neighbor_track(&mut self, forward: bool) {
        if self.opening_project || self.close_prompt_open {
            return;
        }
        let choice = self
            .state
            .as_ref()
            .and_then(|state| selection::neighbor(&state.project, self.selected_track, forward));
        if let Some((index, track, clip)) = choice {
            self.select_track(track);
            self.selected_clip = clip;
            self.track_scroll
                .scroll_to_item(index, gpui::ScrollStrategy::Top);
        }
    }

    pub(super) fn toggle_mute(&mut self, track: TrackId) {
        self.enqueue(Request::Edit(Command::ToggleMute(track)));
    }

    pub(super) fn gain_by(&mut self, track: TrackId, amount: f32) {
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

    pub(super) fn remove_track(&mut self) {
        if let (Some(track), Some(state)) = (self.selected_track, self.state.as_ref()) {
            if state.project.track(track).is_some() {
                self.duplicate_intent.cancel_chain();
                self.enqueue(Request::Edit(Command::RemoveTrack(track)));
                self.selected_track = None;
                self.selected_clip = None;
            }
        }
    }

    pub(super) fn state(&self) -> Option<&Arc<ViewState>> {
        self.state.as_ref()
    }

    pub(super) fn audio_status(&self) -> Option<&Status> {
        self.audio
            .as_ref()
            .map(|audio| audio.control.status.as_ref())
    }

    fn visible_viewport(&self, horizon: u64) -> Viewport {
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

    pub(super) fn set_selected_clip(&mut self, track: TrackId, clip: ClipId) {
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
        self.selected_clip = Some(clip);
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
