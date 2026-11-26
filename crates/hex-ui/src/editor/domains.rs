//! Explicit domain state owners extracted from [`super::EditorView`].
//!
//! Each struct groups cohesive UI state for one editor domain so [`super::EditorView`]
//! coordinates domain owners rather than carrying hundreds of unrelated flat fields.
//! Browser state lives directly on [`super::browser::BrowserState`] and device-panel
//! state lives directly on [`super::device_panel::DevicePanelState`].

use super::{
    PendingSceneDuplicate, arrangement_audio_panel, audio_event_editor, changelog,
    clip_detail_resize, clip_loop, clip_view, console_returns, detail_context, dialogs,
    issue_report, midi_audition, midi_created_selection, midi_creation, midi_drag, midi_group_drag,
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

/// Preferences, tuning, changelog, issue reporting, and missing-media dialog state.
pub(in crate::editor) struct SettingsState {
    pub(in crate::editor) recent_projects: recent_projects::RecentProjectsState,
    pub(in crate::editor) project_directory: project_directory::State,
    pub(in crate::editor) changelog: changelog::State,
    pub(in crate::editor) issue_report: issue_report::State,
    pub(in crate::editor) missing_media: Option<missing_media::Panel>,
    pub(in crate::editor) theme: theme_settings::ThemeSettingsState,
    pub(in crate::editor) audio_device: super::audio_device_settings::State,
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

/// Audio publication, recovery, deferred transport and seek tracking state owned by [`super::EditorView`].
pub(in crate::editor) struct AudioPublicationState {
    pub(in crate::editor) retired_action_outcomes: Option<hex_audio::prepared::ActionOutcomeReader>,
    pub(in crate::editor) retired_action_message: Option<String>,
    pub(in crate::editor) source_reservation: Option<hex_session::SourcePlanReservation>,
    pub(in crate::editor) restart_request_id: u64,
    pub(in crate::editor) pending_restart_request: Option<(u64, u64)>,
    pub(in crate::editor) pending_recovery_generation: Option<u64>,
    /// Output whose default route changed and still awaits its recovery.
    pub(in crate::editor) route_follow: Option<u64>,
    pub(in crate::editor) pending_audio_result: Option<super::PendingEditorAudioResult>,
    pub(in crate::editor) pending_publication_observation:
        Option<super::PendingEditorPublicationObservation>,
    pub(in crate::editor) document_audio_mode: hex_session::DocumentAudioMode,
    pub(in crate::editor) recovery: super::RecoveryState,
    pub(in crate::editor) stopped_playback_restore:
        Option<super::audio_admission::StoppedPlaybackRestore>,
    pub(in crate::editor) latency_rebuild: Option<super::audio_admission::LatencyRebuild>,
    pub(in crate::editor) paused_republish: Option<super::audio_admission::PausedRepublish>,
    // Play requested while the stopped restore renews effects (for example
    // vst3); it starts once that restore settles (Pulse #3784).
    pub(in crate::editor) deferred_play:
        Option<(hex_engine::playback::PlaybackAction, Option<u64>, u64)>,
    // Play requested during a brief sub-frame window where an in-flight edit
    // or audio publication is completing; dispatches once the applied source
    // is ready (Pulse #8948).
    pub(in crate::editor) deferred_toggle_play: Option<super::DeferredTogglePlay>,
    // Seek requested while an already-applied Resident audio revision is
    // waiting for its worker_submitted preroll metadata (Pulse #8948).
    pub(in crate::editor) deferred_metadata_seek: Option<(u64, u64)>,
    pub(in crate::editor) offline_cursor: u64,
    pub(in crate::editor) pending_playback: Option<super::PendingPlayback>,
    pub(in crate::editor) pending_resident_publication: Option<super::PendingResidentPublication>,
    pub(in crate::editor) pending_audio_commands:
        std::collections::VecDeque<super::PendingPlaybackAction>,
    pub(in crate::editor) pending_seek_ticket: Option<u64>,
    pub(in crate::editor) pending_metronome_ticket: Option<u64>,
    pub(in crate::editor) playback_candidates: std::collections::VecDeque<super::PlaybackCandidate>,
    pub(in crate::editor) audible_playback: Option<super::PlaybackCandidate>,
    pub(in crate::editor) resident_tuning_preview: Option<hex_session::service::TuningPreview>,
    pub(in crate::editor) audio_admission: super::AudioAdmissionStatus,
    pub(in crate::editor) prepared_seek: Option<super::arrangement_seek::PendingSeek>,
    pub(in crate::editor) seek_request_serial: u64,
    pub(in crate::editor) latched_seek_intent: Option<super::arrangement_seek::SeekIntent>,
    pub(in crate::editor) retired_seek_result:
        Option<hex_session::arrangement_seek::ArrangementSeekResult>,
    pub(in crate::editor) applied_project_revision: Option<u64>,
    pub(in crate::editor) observed_rejected_action: u64,
    pub(in crate::editor) observed_rejected_revision: u64,
}

impl Default for AudioPublicationState {
    fn default() -> Self {
        Self {
            retired_action_outcomes: None,
            retired_action_message: None,
            source_reservation: None,
            restart_request_id: 0,
            pending_restart_request: None,
            pending_recovery_generation: None,
            route_follow: None,
            pending_audio_result: None,
            pending_publication_observation: None,
            document_audio_mode: hex_session::DocumentAudioMode::Resident,
            recovery: super::RecoveryState::default(),
            stopped_playback_restore: None,
            latency_rebuild: None,
            paused_republish: None,
            deferred_play: None,
            deferred_toggle_play: None,
            deferred_metadata_seek: None,
            offline_cursor: 0,
            pending_playback: None,
            pending_resident_publication: None,
            pending_audio_commands: std::collections::VecDeque::new(),
            pending_seek_ticket: None,
            pending_metronome_ticket: None,
            playback_candidates: std::collections::VecDeque::new(),
            audible_playback: None,
            resident_tuning_preview: None,
            audio_admission: super::AudioAdmissionStatus::default(),
            prepared_seek: None,
            seek_request_serial: 0,
            latched_seek_intent: None,
            retired_seek_result: None,
            applied_project_revision: None,
            observed_rejected_action: 0,
            observed_rejected_revision: 0,
        }
    }
}

/// Timeline paint, lane-bounds, arrangement-map and waveform geometry caches owned by [`super::EditorView`].
pub(in crate::editor) struct TimelineCacheState {
    pub(in crate::editor) clip_content_cache: super::clip_content_cache::TrackCache,
    pub(in crate::editor) track_header_cache: super::track_header_cache::TrackHeaderCache,
    pub(in crate::editor) waveform_overrange_epoch: u64,
    pub(in crate::editor) drag_lanes: std::collections::HashMap<TrackId, super::drag::LaneBounds>,
    pub(in crate::editor) track_drop_lanes:
        std::collections::HashMap<TrackId, super::drag::LaneBounds>,
    pub(in crate::editor) drag_lane_epochs: std::collections::HashMap<TrackId, u64>,
    pub(in crate::editor) arrangement_bounds: Option<super::drag::LaneBounds>,
    pub(in crate::editor) arrangement_bounds_epoch: u64,
    pub(in crate::editor) main_track_lane_bounds: Option<super::drag::LaneBounds>,
    pub(in crate::editor) main_track_lane_epoch: u64,
    pub(in crate::editor) drag_layout_generation: u64,
    pub(in crate::editor) arrangement_map_cache:
        RefCell<Option<Rc<super::arrangement_map::MapCache>>>,
    pub(in crate::editor) focused_arrangement_map_cache:
        RefCell<Option<Rc<super::arrangement_map::MapCache>>>,
    pub(in crate::editor) section_map_cache:
        RefCell<Option<Rc<super::arrangement_map::SectionMapCache>>>,
    pub(in crate::editor) arrangement_clip_index:
        RefCell<super::arrangement_clip_index::ArrangementClipIndex>,
    pub(in crate::editor) clip_title_cache: Rc<RefCell<super::clip_title_cache::ClipTitleCache>>,
    pub(in crate::editor) group_overview_cache: RefCell<super::group_overview::Cache>,
    pub(in crate::editor) overview_waveform_cache: Rc<RefCell<super::overview_waveform::Cache>>,
    pub(in crate::editor) waveform_geometry_cache: Rc<waveform_geometry::WaveformViewCache>,
}

impl TimelineCacheState {
    pub(in crate::editor) fn new(
        waveform_geometry_cache: Rc<waveform_geometry::WaveformViewCache>,
    ) -> Self {
        Self {
            clip_content_cache: super::clip_content_cache::TrackCache::default(),
            track_header_cache: super::track_header_cache::TrackHeaderCache::default(),
            waveform_overrange_epoch: 0,
            drag_lanes: std::collections::HashMap::new(),
            track_drop_lanes: std::collections::HashMap::new(),
            drag_lane_epochs: std::collections::HashMap::new(),
            arrangement_bounds: None,
            arrangement_bounds_epoch: 0,
            main_track_lane_bounds: None,
            main_track_lane_epoch: 0,
            drag_layout_generation: 0,
            arrangement_map_cache: RefCell::new(None),
            focused_arrangement_map_cache: RefCell::new(None),
            section_map_cache: RefCell::new(None),
            arrangement_clip_index: RefCell::new(Default::default()),
            clip_title_cache: Rc::new(RefCell::new(Default::default())),
            group_overview_cache: RefCell::default(),
            overview_waveform_cache: Rc::new(RefCell::new(
                super::overview_waveform::Cache::default(),
            )),
            waveform_geometry_cache,
        }
    }
}

/// Arrangement pointer gestures, loop rail, fades, clip drags, edit cursor and time selection state.
pub(in crate::editor) struct ArrangementInteractionState {
    pub(in crate::editor) loop_rail_gesture: Option<super::loop_rail::LoopRailGesture>,
    pub(in crate::editor) pending_loop: Option<super::loop_rail::PendingLoop>,
    pub(in crate::editor) section_launch_after_loop:
        Option<super::section_launch::SectionLaunchAfterLoop>,
    pub(in crate::editor) loop_rail_cursor: gpui::CursorStyle,
    pub(in crate::editor) arrangement_pan: Option<super::ArrangementPan>,
    pub(in crate::editor) scrub: super::scrub::ScrubState,
    pub(in crate::editor) zoom_wheel_accum: f32,
    pub(in crate::editor) hovered_arrangement_clip:
        Option<super::track_lane_canvas::HoveredArrangementClip>,
    pub(in crate::editor) active_clip_drag: Option<super::drag::ClipDrag>,
    pub(in crate::editor) clip_move_audition: Option<super::drag::MoveAudition>,
    pub(in crate::editor) clip_move_gesture: u64,
    pub(in crate::editor) active_trim_drag: Option<trim::TrimDrag>,
    pub(in crate::editor) pending_trim: Option<trim::PendingTrim>,
    pub(in crate::editor) fade_key_held: bool,
    /// Option held: the right clip header edge offers repeat-extend.
    pub(in crate::editor) option_held: bool,
    pub(in crate::editor) time_selection_key_held: bool,
    pub(in crate::editor) active_fade_drag: Option<super::fade_handles::FadeDrag>,
    pub(in crate::editor) pending_fade: Option<super::fade_handles::PendingFade>,
    pub(in crate::editor) clip_gain: super::clip_gain::State,
    pub(in crate::editor) pending_clip_activation: Option<super::activation::PendingClipActivation>,
    pub(in crate::editor) hovered_fade_clip: Option<(u64, ClipId)>,
    pub(in crate::editor) hovered_fade_handle: Option<super::fade_handles::Hover>,
    pub(in crate::editor) pointer_down: Option<super::drag::PointerDown>,
    pub(in crate::editor) edit_cursor: Option<super::edit_cursor::EditCursor>,
    /// Transient Arrangement play-start lock; never seeks the active transport.
    pub(in crate::editor) locked_play_start: Option<super::edit_cursor::EditCursor>,
    pub(in crate::editor) locked_play_start_drag: Option<super::edit_cursor::LockedPlayStartDrag>,
    pub(in crate::editor) bottom_play_start_marker_bounds:
        std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>,
    pub(in crate::editor) selected_clip_play_start:
        std::cell::Cell<Option<super::selection_play_start::SelectedClipPlayStart>>,
    pub(in crate::editor) play_start_at_beginning: bool,
    pub(in crate::editor) escape_stop_armed: bool,
    /// The selected clip a point cursor was deliberately placed inside; that
    /// cursor then outranks the clip's play start (see `selected_arrangement_play_start`).
    pub(in crate::editor) edit_cursor_in_selected_clip: Option<ClipId>,
    /// An acknowledged drop places a point independently of the clip or Main
    /// selection. A new selection intent ends that precedence.
    pub(in crate::editor) clip_drop_cursor_epoch: Option<u64>,
    /// The selected clip whose in-clip cursor a loop enable retired; the loop
    /// start then keeps outranking that clip's play start until a new click.
    pub(in crate::editor) loop_start_over_clip: Option<ClipId>,
    /// Successful selection-loop start, fenced by the user's selection intent.
    pub(in crate::editor) selection_loop_start: Option<(u64, hex_project::LoopRegion)>,
    pub(in crate::editor) clip_paint_gesture: Option<super::clip_paint::ClipPaintGesture>,
    pub(in crate::editor) time_selection: Option<super::time_selection::TimeSelection>,
    pub(in crate::editor) time_selection_project_scope: bool,
    pub(in crate::editor) selection_intent_epoch: Option<u64>,
    pub(in crate::editor) time_selection_gesture:
        Option<super::time_selection::TimeSelectionGesture>,
    pub(in crate::editor) time_range_move_gesture:
        Option<super::time_selection::TimeRangeMoveGesture>,
    pub(in crate::editor) pending_time_range_move:
        Option<super::time_selection::PendingTimeRangeMove>,
    pub(in crate::editor) ahead_time_range_move:
        Option<super::time_selection::PendingTimeRangeMove>,
    pub(in crate::editor) suppress_cursor_click: bool,
}

impl Default for ArrangementInteractionState {
    fn default() -> Self {
        Self {
            loop_rail_gesture: None,
            pending_loop: None,
            section_launch_after_loop: None,
            loop_rail_cursor: gpui::CursorStyle::Crosshair,
            arrangement_pan: None,
            scrub: super::scrub::ScrubState::default(),
            zoom_wheel_accum: 0.0,
            hovered_arrangement_clip: None,
            active_clip_drag: None,
            clip_move_audition: None,
            clip_move_gesture: 0,
            active_trim_drag: None,
            pending_trim: None,
            fade_key_held: false,
            option_held: false,
            time_selection_key_held: false,
            active_fade_drag: None,
            pending_fade: None,
            clip_gain: super::clip_gain::State::default(),
            pending_clip_activation: None,
            hovered_fade_clip: None,
            hovered_fade_handle: None,
            pointer_down: None,
            edit_cursor: None,
            locked_play_start: None,
            locked_play_start_drag: None,
            bottom_play_start_marker_bounds: Default::default(),
            selected_clip_play_start: Default::default(),
            play_start_at_beginning: false,
            escape_stop_armed: false,
            edit_cursor_in_selected_clip: None,
            clip_drop_cursor_epoch: None,
            loop_start_over_clip: None,
            selection_loop_start: None,
            clip_paint_gesture: None,
            time_selection: None,
            time_selection_project_scope: false,
            selection_intent_epoch: Some(0),
            time_selection_gesture: None,
            time_range_move_gesture: None,
            pending_time_range_move: None,
            ahead_time_range_move: None,
            suppress_cursor_click: false,
        }
    }
}
