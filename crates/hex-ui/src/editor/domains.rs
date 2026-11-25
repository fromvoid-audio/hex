//! Explicit domain state owners extracted from [`super::EditorView`].
//!
//! Each struct groups cohesive UI state for one editor domain so [`super::EditorView`]
//! coordinates domain owners rather than carrying hundreds of unrelated flat fields.
//! Browser state lives directly on [`super::browser::BrowserState`] and device-panel
//! state lives directly on [`super::device_panel::DevicePanelState`].

use super::{
    PendingSceneDuplicate, arrangement_audio_panel, audio_event_editor, changelog,
    clip_detail_resize, clip_loop, clip_view, console_returns, detail_context, dialogs,
    midi_audition, midi_created_selection, midi_creation, midi_drag, midi_group_drag,
    midi_loop_notes, midi_origin_view, midi_pan, midi_pending_drag,
    midi_pitch_viewport::PitchViewport, midi_pitch_zoom, midi_recording, midi_ruler, midi_scene,
    midi_selection, midi_step, midi_stroke, midi_velocity_randomize, midi_velocity_ui,
    missing_media, musical_audio_controls, playback_settings, project_directory,
    project_rate_settings, recent_projects, save, session_route_view, theme_settings,
    track_defaults, trim, tuning, update_flow, waveform_geometry,
};
use gpui::{FocusHandle, Subscription};
use hex_project::{ClipId, SceneId, TrackId};
use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Arc};

/// Piano roll and MIDI clip editing state owned by [`super::EditorView`].
pub(in crate::editor) struct MidiEditorState {
    pub(in crate::editor) selection: midi_selection::MidiSelection,
    pub(in crate::editor) note_gesture: Option<midi_drag::NoteGesture>,
    pub(in crate::editor) group_gesture: Option<midi_group_drag::GroupGesture>,
    pub(in crate::editor) pending_drag: midi_pending_drag::PendingQueue,
    pub(in crate::editor) origin_history: midi_origin_view::State,
    pub(in crate::editor) creation: Option<midi_creation::Creation>,
    pub(in crate::editor) stroke: Option<midi_stroke::MidiStroke>,
    pub(in crate::editor) draw_mode: bool,
    pub(in crate::editor) draw_pitch_locked: bool,
    /// Source-relative insertion beat; reset with the existing `piano_view_clip` owner.
    /// Pure view state: never transport position or a canonical project edit.
    pub(in crate::editor) marker: Option<u64>,
    pub(in crate::editor) step: midi_step::StepEntry,
    pub(in crate::editor) velocity: midi_velocity_ui::State,
    pub(in crate::editor) velocity_randomize: midi_velocity_randomize::State,
    pub(in crate::editor) pan: midi_pan::State,
    pub(in crate::editor) ruler: midi_ruler::State,
    pub(in crate::editor) time_selection: midi_ruler::TimeSelection,
    pub(in crate::editor) audition: midi_audition::MidiAudition,
    pub(in crate::editor) pending_created_note_selection: Option<midi_created_selection::Pending>,
    pub(in crate::editor) marquee: Option<midi_selection::Marquee>,
    pub(in crate::editor) scene_cache: midi_scene::MidiSceneCache,
    pub(in crate::editor) hit_cycle: Option<midi_scene::HitCycle>,
    /// Only suppresses redundant pointer invalidations; paint resolves current geometry.
    pub(in crate::editor) hover: Option<midi_scene::NoteHover>,
    pub(in crate::editor) piano_view_clip: Option<(u64, ClipId)>,
    pub(in crate::editor) piano_zoom_wheel_accum: f32,
    pub(in crate::editor) piano_pitch_scroll_accum: f32,
    pub(in crate::editor) piano_beat_start: i128,
    pub(in crate::editor) piano_beat_span: hex_project::BeatTime,
    pub(in crate::editor) piano_pitch: PitchViewport,
    pub(in crate::editor) piano_pitch_zoom: Option<midi_pitch_zoom::Zoom>,
    pub(in crate::editor) piano_pitch_grid_height: f32,
    pub(in crate::editor) piano_pitch_note_fit: bool,
    pub(in crate::editor) snap_grid: midi_drag::MidiGrid,
    pub(in crate::editor) focus: FocusHandle,
    pub(in crate::editor) _focus_lifecycle: [Subscription; 3],
    pub(in crate::editor) visible_notes: RefCell<midi_loop_notes::VisibleNotesCache>,
    pub(in crate::editor) tile_images: Rc<RefCell<midi_loop_notes::TileImages>>,
    pub(in crate::editor) recorder: midi_recording::Recorder,
    pub(in crate::editor) active_trim_drag: Option<trim::MidiTrimDrag>,
}

impl MidiEditorState {
    pub(in crate::editor) fn new(focus: FocusHandle, focus_lifecycle: [Subscription; 3]) -> Self {
        Self {
            selection: midi_selection::MidiSelection::default(),
            note_gesture: None,
            group_gesture: None,
            pending_drag: midi_pending_drag::PendingQueue::default(),
            origin_history: midi_origin_view::State::default(),
            creation: None,
            stroke: None,
            draw_mode: false,
            draw_pitch_locked: true,
            marker: None,
            step: midi_step::StepEntry::default(),
            velocity: midi_velocity_ui::State::default(),
            velocity_randomize: midi_velocity_randomize::State::default(),
            pan: midi_pan::State::default(),
            ruler: midi_ruler::State::default(),
            time_selection: midi_ruler::TimeSelection::default(),
            audition: midi_audition::MidiAudition::default(),
            pending_created_note_selection: None,
            marquee: None,
            scene_cache: midi_scene::MidiSceneCache::default(),
            hit_cycle: None,
            hover: None,
            piano_view_clip: None,
            piano_zoom_wheel_accum: 0.0,
            piano_pitch_scroll_accum: 0.0,
            piano_beat_start: 0,
            piano_beat_span: hex_project::BeatTime::from_quarters(4)
                .unwrap_or(hex_project::BeatTime::ONE),
            piano_pitch: PitchViewport::DEFAULT,
            piano_pitch_zoom: None,
            piano_pitch_grid_height: 0.0,
            piano_pitch_note_fit: false,
            snap_grid: midi_drag::MidiGrid::default(),
            focus,
            _focus_lifecycle: focus_lifecycle,
            visible_notes: RefCell::default(),
            tile_images: Rc::default(),
            recorder: midi_recording::Recorder::default(),
            active_trim_drag: None,
        }
    }
}

/// Audio clip detail and clip-local event editing state owned by [`super::EditorView`].
pub(in crate::editor) struct AudioEventEditorState {
    pub(in crate::editor) event: audio_event_editor::State,
    pub(in crate::editor) focus: FocusHandle,
    pub(in crate::editor) _blur: Subscription,
    pub(in crate::editor) clip_controls: arrangement_audio_panel::controls::Controls,
    pub(in crate::editor) sample_bpm_control: Option<musical_audio_controls::sample_bpm::Binding>,
    pub(in crate::editor) clip_loop: clip_loop::State,
    pub(in crate::editor) clip_view_open: bool,
    pub(in crate::editor) detail_body_height: f32,
    pub(in crate::editor) detail_resize: clip_detail_resize::State,
    pub(in crate::editor) waveform_geometry_cache: Rc<waveform_geometry::WaveformViewCache>,
    pub(in crate::editor) point_page: usize,
    pub(in crate::editor) detail_context: Option<detail_context::DetailContext>,
}

impl AudioEventEditorState {
    pub(in crate::editor) fn new(
        focus: FocusHandle,
        blur: Subscription,
        waveform_geometry_cache: Rc<waveform_geometry::WaveformViewCache>,
    ) -> Self {
        Self {
            event: audio_event_editor::State::default(),
            focus,
            _blur: blur,
            clip_controls: Default::default(),
            sample_bpm_control: None,
            clip_loop: clip_loop::State::default(),
            clip_view_open: true,
            detail_body_height: clip_view::CLIP_DETAIL_BODY_HEIGHT,
            detail_resize: clip_detail_resize::State::default(),
            waveform_geometry_cache,
            point_page: 0,
            detail_context: None,
        }
    }
}

/// Session grid, scene/cell selection, console returns, and routing panel state.
pub(in crate::editor) struct SessionGridState {
    pub(in crate::editor) selected_scene: Option<SceneId>,
    pub(in crate::editor) selected_cell: Option<(TrackId, SceneId)>,
    pub(in crate::editor) copy_source: Option<ClipId>,
    pub(in crate::editor) track_start: usize,
    pub(in crate::editor) visible_columns: usize,
    pub(in crate::editor) quantization: hex_engine::LaunchQuantization,
    pub(in crate::editor) scene_scroll: gpui::UniformListScrollHandle,
    pub(in crate::editor) routes: session_route_view::RouteView,
    pub(in crate::editor) pending_scene_duplicate: Option<PendingSceneDuplicate>,
    pub(in crate::editor) pending_session_ticket: Option<(u64, u64)>,
    pub(in crate::editor) observed_capability: Option<u32>,
    pub(in crate::editor) capability_message: Option<String>,
    pub(in crate::editor) console_returns: console_returns::State,
    pub(in crate::editor) focused_return: Option<hex_project::ReturnId>,
    pub(in crate::editor) send_menu_track: Option<TrackId>,
    pub(in crate::editor) routing_open: bool,
    pub(in crate::editor) routing_page: usize,
}

impl Default for SessionGridState {
    fn default() -> Self {
        Self {
            selected_scene: None,
            selected_cell: None,
            copy_source: None,
            track_start: 0,
            visible_columns: 1,
            quantization: hex_engine::LaunchQuantization::OneBar,
            scene_scroll: gpui::UniformListScrollHandle::new(),
            routes: session_route_view::RouteView::default(),
            pending_scene_duplicate: None,
            pending_session_ticket: None,
            observed_capability: None,
            capability_message: None,
            console_returns: console_returns::State::default(),
            focused_return: None,
            send_menu_track: None,
            routing_open: false,
            routing_page: 0,
        }
    }
}

/// Preferences, tuning, changelog, and missing-media dialog state.
pub(in crate::editor) struct SettingsState {
    pub(in crate::editor) recent_projects: recent_projects::RecentProjectsState,
    pub(in crate::editor) project_directory: project_directory::State,
    pub(in crate::editor) changelog: changelog::State,
    pub(in crate::editor) missing_media: Option<missing_media::Panel>,
    pub(in crate::editor) theme: theme_settings::ThemeSettingsState,
    pub(in crate::editor) project_rate: project_rate_settings::State,
    pub(in crate::editor) audio_import: super::audio_import_settings::State,
    pub(in crate::editor) playback: playback_settings::State,
    pub(in crate::editor) track_defaults: track_defaults::State,
    pub(in crate::editor) tuning: tuning::TuningState,
}

/// Project open/save, export, bounce, batch import, close, and update workflow state.
#[derive(Default)]
pub(in crate::editor) struct PersistenceState {
    pub(in crate::editor) opening_project: bool,
    pub(in crate::editor) project_open: dialogs::project_open::State,
    pub(in crate::editor) pending_open_path: Option<PathBuf>,
    pub(in crate::editor) pending_open_mode: Option<hex_session::DocumentAudioMode>,
    pub(in crate::editor) requested_audio_mode: Option<hex_session::DocumentAudioMode>,
    pub(in crate::editor) close_prompt_open: bool,
    pub(in crate::editor) automation_close_preflight: super::close::AutomationClosePreflight,
    pub(in crate::editor) export_pending: bool,
    pub(in crate::editor) bounce_control: Option<hex_session::service::ExportControl>,
    pub(in crate::editor) bounce_fraction: f32,
    /// `b` is held during the active Arrangement clip drag; release bounces.
    pub(in crate::editor) drag_bounce_held: bool,
    pub(in crate::editor) pending_bounce_drop: Option<super::bounce::PendingBounceDrop>,
    pub(in crate::editor) export_control: Option<hex_session::service::ExportControl>,
    pub(in crate::editor) export_progress: Option<hex_session::service::ExportProgress>,
    pub(in crate::editor) export_window: Option<gpui::AnyWindowHandle>,
    pub(in crate::editor) export_dialog_open: bool,
    pub(in crate::editor) saving_project: bool,
    pub(in crate::editor) update_flow: update_flow::State,
    pub(in crate::editor) save_dialog_open: bool,
    pub(in crate::editor) document_format_upgrade_required: bool,
    pub(in crate::editor) format_upgrade_prompt: Option<save::FormatUpgradeIntent>,
    pub(in crate::editor) committed_copy: save::CommittedCopy,
    pub(in crate::editor) audio_batch:
        Option<Arc<hex_session::import_batch_decode::AudioBatchControl>>,
    pub(in crate::editor) audio_batch_progress:
        Option<hex_session::import_batch_decode::AudioBatchProgress>,
}

/// Test-only editor command/drag observation probes.
#[cfg(test)]
#[derive(Default)]
pub(in crate::editor) struct EditorTestProbes {
    pub(in crate::editor) drag_move_event_count: usize,
    pub(in crate::editor) last_enqueued_edit: Option<hex_project::Command>,
    pub(in crate::editor) last_enqueued_clip_conversion: Option<(u64, u64, ClipId, bool)>,
    pub(in crate::editor) last_enqueued_native_device_add: Option<(
        TrackId,
        hex_engine::NativeDeviceClass,
        hex_lattice_contract::DeviceTypeId,
    )>,
    pub(in crate::editor) last_enqueued_native_device_replace: Option<(
        hex_project::DeviceId,
        hex_engine::NativeDeviceClass,
        hex_lattice_contract::DeviceTypeId,
    )>,
    pub(in crate::editor) last_enqueued_arrangement_device_drop:
        Option<hex_session::service::ArrangementDeviceDestination>,
    pub(in crate::editor) last_enqueued_device_track_gain: Option<Option<f32>>,
    pub(in crate::editor) last_enqueued_native_device_drop: Option<super::EnqueuedNativeDeviceDrop>,
    pub(in crate::editor) last_time_request: Option<(&'static str, Option<(TrackId, u64)>)>,
    pub(in crate::editor) last_transport_command: Option<hex_engine::playback::PlaybackCommand>,
    pub(in crate::editor) last_trim_release_snap: Option<bool>,
    pub(in crate::editor) last_enqueued_import_to_track: Option<(PathBuf, TrackId, u64, bool)>,
    pub(in crate::editor) last_enqueued_import_to_new_track:
        Option<(PathBuf, Option<TrackId>, u64)>,
}
