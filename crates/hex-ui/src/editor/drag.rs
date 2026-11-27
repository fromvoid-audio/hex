mod geometry;
mod lane_bounds;
use geometry::lane_contains_pointer;

fn lane_or_leading_buffer_contains_pointer(
    pointer_x: f32,
    pointer_y: f32,
    lane: LaneBounds,
) -> bool {
    super::arrangement::timeline_x_in_bounds_or_buffer(pointer_x, lane.left, lane.width)
        && pointer_y.is_finite()
        && lane.top.is_finite()
        && lane.height.is_finite()
        && lane.height > 0.0
        && pointer_y >= lane.top
        && pointer_y < lane.top + lane.height
}
pub(super) use geometry::{LaneBounds, clip_snap_edge, drop_start, grab_offset_from_pointer};
#[cfg(all(test, feature = "ui-test-support"))]
pub(super) use lane_bounds::ClipSnapEdge;

use super::musical_time::SnapGrid;
use super::viewport::Viewport;
use gpui::{Context, Window};
use hex_project::Command;
use hex_project::{ClipId, Project, TrackId, TrimEdge};
use hex_session::service::{AudioPreview, Request};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

mod logic;
pub(super) use logic::{
    ClipDrag, ClipDropDestination, ClipPointerKey, ClipPosition, DropPreview, DropState,
    MoveAudition, PointerDown, group_position, is_current, is_noop, preview_for, rectangle_part,
};
use logic::{
    candidate_position, context_is_current, copy_command, copy_to_new_track_command, move_command,
    move_to_new_track_command,
};

/// In-window drag payload for library audio, a retained take, or a project asset.
#[derive(Clone)]
pub(super) struct BrowserAudioDrag {
    pub(super) project_generation: u64,
    pub(super) revision: u64,
    pub(super) browser_generation: Option<u64>,
    pub(super) sample_rate: u32,
    pub(super) path: std::path::PathBuf,
    pub(super) project_asset: Option<hex_project::AssetId>,
    pub(super) label: String,
    pub(super) viewport_start: u64,
    pub(super) viewport_length: u64,
    pub(super) scroll: super::drag_autoscroll::DragScroll,
    pub(super) snap_grid: SnapGrid,
    pub(super) snap_modifier: Rc<Cell<bool>>,
    pub(super) copy_modifier: Rc<Cell<bool>>,
    pub(super) metadata: Rc<std::cell::RefCell<Option<Rc<AudioPreview>>>>,
    pub(super) metadata_error: Rc<std::cell::RefCell<Option<String>>>,
    pub(super) preview: Rc<Cell<Option<DropPreview>>>,
    pub(super) destination: Rc<Cell<Option<BrowserDropDestination>>>,
    pub(super) insertion_line_y: Rc<Cell<Option<f32>>>,
    pub(super) lane_hovered: Rc<Cell<bool>>,
    pub(super) last_pointer: Rc<Cell<Option<(f32, f32)>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NativeDeviceDropTarget {
    pub(super) track: TrackId,
    /// Canonical ordered-chain node before which the device should land.
    /// `None` appends existing devices; new browser instruments default to front.
    pub(super) before: Option<hex_project::DeviceId>,
    /// Exact occupied slot for a browser instrument or a compatible VST3 pad.
    pub(super) replace: Option<hex_project::DeviceId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum NativeDeviceDragSource {
    Browser {
        class: hex_engine::NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
    },
    PluginEffect(std::sync::Arc<hex_plugin_catalog::PluginSelection>),
    Existing {
        device: hex_project::DeviceId,
        class: hex_engine::NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
    },
}

impl NativeDeviceDragSource {
    pub(super) fn is_catalog_vst3_effect(&self) -> bool {
        matches!(self, Self::PluginEffect(selection) if selection.role == hex_plugin_catalog::PluginRole::Effect)
    }
    pub(super) fn is_catalog_vst3_instrument(&self) -> bool {
        matches!(self, Self::PluginEffect(selection) if selection.role == hex_plugin_catalog::PluginRole::Instrument)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectBusDropSlot {
    Before(Option<hex_project::DeviceId>),
    Replace(hex_project::DeviceId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReturnDeviceDropTarget {
    pub(super) owner: super::device_panel::EffectBusOwner,
    pub(super) slot: EffectBusDropSlot,
}

impl ReturnDeviceDropTarget {
    pub(super) fn anchor(self) -> Option<hex_project::DeviceId> {
        match self.slot {
            EffectBusDropSlot::Before(before) => before,
            EffectBusDropSlot::Replace(device) => Some(device),
        }
    }
}

/// Metadata-only payload shared by catalog drops and ordered-chain moves.
#[derive(Clone, Debug)]
pub(super) struct NativeDeviceDrag {
    pub(super) project_generation: u64,
    pub(super) revision: u64,
    pub(super) source: NativeDeviceDragSource,
    pub(super) destination: Rc<Cell<Option<NativeDeviceDropTarget>>>,
    pub(super) return_destination: Rc<Cell<Option<ReturnDeviceDropTarget>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BrowserDropDestination {
    Console(hex_session::import_batch::ConsoleDestination),
    ExistingTrack {
        track: TrackId,
        start: u64,
    },
    NewTrack {
        before_track: Option<TrackId>,
        start: u64,
    },
}

pub(super) struct BrowserDropCandidate<'a> {
    pub project: &'a Project,
    pub viewport_start: u64,
    pub viewport_length: u64,
    pub snap_grid: SnapGrid,
    pub snap_modifier: bool,
    pub pointer_x: f32,
    pub lane: LaneBounds,
    pub duration: u64,
    pub target_track: TrackId,
}

pub(super) fn browser_candidate(candidate: BrowserDropCandidate<'_>) -> Option<DropPreview> {
    let start = browser_start(
        candidate.project,
        candidate.viewport_start,
        candidate.viewport_length,
        candidate
            .snap_grid
            .with_platform_modifier(candidate.snap_modifier),
        candidate.pointer_x,
        candidate.lane,
    )?;
    preview_for(
        candidate.project,
        candidate.target_track,
        ClipPosition::Audio {
            start,
            duration: candidate.duration,
        },
    )
}

pub(super) fn browser_start(
    project: &Project,
    viewport_start: u64,
    viewport_length: u64,
    snap_grid: SnapGrid,
    pointer_x: f32,
    lane: LaneBounds,
) -> Option<u64> {
    let pointer_frame = drop_start(pointer_x, lane, viewport_start, viewport_length, 0)?;
    snap_grid.snap_frame(
        project,
        pointer_frame,
        Viewport {
            start: viewport_start,
            length: viewport_length,
        },
        lane.width,
    )
}

impl super::EditorView {
    pub(super) fn remember_pointer_down(
        &mut self,
        generation: u64,
        clip: ClipId,
        edge: Option<TrimEdge>,
        x: f32,
    ) {
        self.interaction.pointer_down = PointerDown::begin(
            ClipPointerKey { generation, clip },
            rectangle_part(edge),
            x,
            true,
        );
    }

    pub(super) fn take_pointer_down(
        &mut self,
        generation: u64,
        clip: ClipId,
        edge: Option<TrimEdge>,
    ) -> Option<PointerDown> {
        PointerDown::take_matching(
            &mut self.interaction.pointer_down,
            ClipPointerKey { generation, clip },
            rectangle_part(edge),
        )
    }

    pub(super) fn clear_pointer_down(
        &mut self,
        generation: u64,
        clip: ClipId,
        edge: Option<TrimEdge>,
    ) {
        PointerDown::take_matching(
            &mut self.interaction.pointer_down,
            ClipPointerKey { generation, clip },
            rectangle_part(edge),
        );
    }

    pub(super) fn drag_start_is_current(&self, drag: &ClipDrag) -> bool {
        let current = (|| {
            if drag.group.borrow().ids.len() > 1
                && drag.group.borrow().selection_epoch != Some(self.clip_selection.epoch)
            {
                return false;
            }
            // Source validity follows the installed canonical state. Harmless
            // worker work does not retire a held gesture; existing-track release
            // also validates this captured revision on the worker before editing.
            if self.persistence.opening_project || self.persistence.close_prompt_open {
                return false;
            }
            let Some(state) = self.state.as_ref() else {
                return false;
            };
            let viewport = self.visible_viewport(state.timeline_frames);
            let drop_state = DropState {
                project_generation: self.project_generation,
                revision: state.revision,
                project: &state.project,
                target_track: drag.source_track,
                position: drag.position,
                viewport_start: viewport.start,
                viewport_length: viewport.length,
                snap_grid: self.snap_grid,
            };
            let group = drag.group.borrow();
            if group.ids.len() > 1 {
                let length = match drag.position {
                    ClipPosition::Audio { duration, .. } => duration,
                    ClipPosition::Midi { length, .. } => length.raw(),
                };
                context_is_current(drag, &drop_state)
                    && group.capture.as_ref().is_some_and(|capture| {
                        capture.anchor_matches(
                            &state.project,
                            drag.clip,
                            drag.source_track,
                            group_position(drag.position),
                            length,
                        )
                    })
            } else {
                is_current(drag, drop_state)
            }
        })();
        if !current {
            // Context retirement happens inside the shared payload owner; a
            // GPUI payload clone cannot keep a usable source capture alive.
            drag.group.borrow_mut().capture = None;
        }
        current
    }

    pub(super) fn drag_candidate(
        &self,
        drag: &ClipDrag,
        target_track: TrackId,
        pointer_x: f32,
        lane: LaneBounds,
    ) -> Option<DropPreview> {
        // Every unsuccessful projection invalidates ALL row destinations,
        // including early stale/no-state/position failures and final bounds.
        let preview = (|| {
            if !self.drag_start_is_current(drag) {
                return None;
            }
            let state = self.state.as_ref()?;
            let position =
                candidate_position(&state.project, drag, Some(target_track), pointer_x, lane)?;
            if drag.group.borrow().ids.len() > 1 {
                let mut group = drag.group.borrow_mut();
                group
                    .capture
                    .as_mut()?
                    .update(&state.project, target_track, group_position(position))
                    .ok()?;
            }
            let position = if drag.group.borrow().ids.len() > 1 {
                let group = drag.group.borrow();
                match &group
                    .previews()
                    .iter()
                    .find(|p| p.id() == drag.clip)?
                    .content
                {
                    hex_project::ClipGroupContent::Audio(c) => ClipPosition::Audio {
                        start: c.start(),
                        duration: c.duration(),
                    },
                    hex_project::ClipGroupContent::Midi(c) => ClipPosition::Midi {
                        start: c.start(),
                        length: c.length(),
                    },
                }
            } else {
                position
            };
            preview_for(&state.project, target_track, position)
        })();
        if preview.is_none() {
            drag.group.borrow_mut().invalidate();
        }
        preview
    }

    pub(super) fn clip_drag_drop_position(
        &self,
        drag: &ClipDrag,
        pointer_x: f32,
        pointer_y: f32,
    ) -> (Option<DropPreview>, bool) {
        drag.group.borrow_mut().invalidate();
        let Some(pointer_x) = PointerDown::pointer(&drag.capture, pointer_x) else {
            drag.destination.set(None);
            drag.insertion_line_y.set(None);
            return (None, false);
        };
        if !self.drag_start_is_current(drag) {
            drag.destination.set(None);
            drag.insertion_line_y.set(None);
            return (None, false);
        }
        let Some(state) = self.state.as_ref() else {
            drag.destination.set(None);
            drag.insertion_line_y.set(None);
            return (None, false);
        };
        if let Some((before_track, timeline, line_y)) =
            self.painted_track_insertion_target(pointer_x, pointer_y)
        {
            if drag.group.borrow().ids.len() > 1 {
                drag.group.borrow_mut().invalidate();
                drag.destination.set(None);
                drag.insertion_line_y.set(None);
                return (None, true);
            }
            let preview = candidate_position(&state.project, drag, None, pointer_x, timeline)
                .and_then(|position| preview_for(&state.project, drag.source_track, position));
            let Some(preview) = preview else {
                drag.destination.set(None);
                drag.insertion_line_y.set(None);
                return (None, false);
            };
            drag.destination.set(Some(ClipDropDestination::NewTrack {
                before_track,
                position: preview.position,
                start_frame: preview.start_frame,
            }));
            drag.insertion_line_y.set(Some(line_y));
            return (None, true);
        }
        if let Some((track, lane)) = self.hovered_drag_target(pointer_x, pointer_y) {
            let preview = self.drag_candidate(drag, track, pointer_x, lane);
            drag.destination
                .set(preview.map(|_| ClipDropDestination::ExistingTrack(track)));
            drag.insertion_line_y.set(None);
            return (preview, true);
        }
        drag.destination.set(None);
        drag.insertion_line_y.set(None);
        (None, false)
    }

    /// Track destinations include attached automation rows, while clip editing
    /// continues to use the separately measured body bounds in `drag_lanes`.
    /// Both bounds publish together under the current painted body's epoch.
    pub(super) fn painted_track_drop_lane(&self, track: TrackId) -> Option<LaneBounds> {
        if !self.arrangement_track_visible(track)
            || self.timeline_caches.drag_lane_epochs.get(&track)
                != Some(&self.timeline_caches.drag_layout_generation)
        {
            return None;
        }
        self.timeline_caches.track_drop_lanes.get(&track).copied()
    }
    pub(super) fn hovered_track_drop_target(
        &self,
        x: f32,
        y: f32,
    ) -> Option<(TrackId, LaneBounds)> {
        self.timeline_caches.drag_lanes.keys().find_map(|track| {
            let lane = self.painted_track_drop_lane(*track)?;
            lane_or_leading_buffer_contains_pointer(x, y, lane).then_some((*track, lane))
        })
    }

    pub(super) fn hovered_drag_target(
        &self,
        pointer_x: f32,
        pointer_y: f32,
    ) -> Option<(TrackId, LaneBounds)> {
        self.timeline_caches
            .drag_lanes
            .iter()
            .filter(|(track, _)| {
                self.arrangement_track_visible(**track)
                    && self.timeline_caches.drag_lane_epochs.get(track)
                        == Some(&self.timeline_caches.drag_layout_generation)
            })
            .find_map(|(track, lane)| {
                lane_or_leading_buffer_contains_pointer(pointer_x, pointer_y, *lane)
                    .then_some((*track, *lane))
            })
    }

    pub(super) fn retire_active_clip_drag(&mut self) -> bool {
        self.cancel_held_clip_move_audition();
        self.persistence.drag_bounce_held = false;
        let Some(drag) = self.interaction.active_clip_drag.take() else {
            return false;
        };
        drag.retire();
        true
    }

    pub(super) fn clear_clip_drag(&mut self, generation: u64, clip: ClipId) {
        if self
            .interaction
            .active_clip_drag
            .as_ref()
            .is_some_and(|drag| drag.project_generation == generation && drag.clip == clip)
        {
            self.retire_active_clip_drag();
        }
    }

    pub(super) fn begin_clip_drag(&mut self, drag: &ClipDrag) {
        if drag.group.borrow().ids.is_empty() {
            drag.group.borrow_mut().ids = if self.clip_is_selected(drag.clip) {
                self.selected_clip_ids()
            } else {
                vec![drag.clip]
            };
            drag.group.borrow_mut().selection_epoch = Some(self.clip_selection.epoch);
        }
        if self
            .interaction
            .active_clip_drag
            .as_ref()
            .is_some_and(|active| Rc::ptr_eq(&active.group, &drag.group))
        {
            return;
        }
        self.retire_active_clip_drag();
        if drag.group.borrow().ids.len() > 1 {
            let capture = self.state.as_ref().and_then(|state| {
                hex_project::ClipGroupMovePreview::capture(
                    &state.project,
                    &drag.group.borrow().ids,
                    drag.clip,
                )
                .ok()
            });
            drag.group.borrow_mut().capture = capture;
        }
        if let Some(gesture) = self.interaction.clip_move_gesture.checked_add(1) {
            self.interaction.clip_move_gesture = gesture;
            self.interaction.clip_move_audition = Some(MoveAudition {
                clips: drag.group.borrow().ids.clone().into(),
                gesture,
                sequence: 0,
                new_track_defaults: None,
                target: None,
            });
        }
        self.device_panel.active_native_drag = None;
        drag.group.borrow_mut().source_project = self
            .state
            .as_ref()
            .map(|s| std::sync::Arc::clone(&s.project));
        self.interaction.active_clip_drag = Some(drag.clone());
        self.browser.active_audio_drag = None;
    }

    /// Submit only changed, valid targets. The worker owns preparation and
    /// publication; this control state never treats submission as application.
    pub(super) fn sync_held_clip_move_audition(&mut self) -> bool {
        let Some(drag) = self.interaction.active_clip_drag.as_ref() else {
            return false;
        };
        let eligible = self.mode == super::EditorMode::Arrangement
            && self.audio_pub.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && !self.persistence.drag_bounce_held
            && self
                .audio_status()
                .is_some_and(|status| status.playing.load(std::sync::atomic::Ordering::Acquire))
            && self.drag_start_is_current(drag);
        let mode = if drag.alias_copy() {
            hex_session::service::ArrangementMoveMode::AliasCopy
        } else if drag.copy_modifier.get() {
            hex_session::service::ArrangementMoveMode::Copy
        } else {
            hex_session::service::ArrangementMoveMode::Move
        };
        let target = if eligible {
            match (drag.destination.get(), drag.preview.get()) {
                (Some(ClipDropDestination::ExistingTrack(track)), Some(preview))
                    if preview.target_track == track
                        && !(drag.source_track == track && drag.position == preview.position) =>
                {
                    Some((
                        hex_session::service::ArrangementMoveDestination::Existing(track),
                        group_position(preview.position),
                        mode,
                    ))
                }
                (
                    Some(ClipDropDestination::NewTrack {
                        before_track,
                        position,
                        ..
                    }),
                    _,
                ) if drag.group.borrow().ids.len() == 1 => {
                    if let Some(audition) = self.interaction.clip_move_audition.as_mut() {
                        if audition.new_track_defaults.is_none()
                            && let (Ok(gain), Some(state)) = (
                                self.settings.track_defaults.captured_gain(),
                                self.state.as_ref(),
                            )
                        {
                            audition.new_track_defaults = Some((
                                format!("track {}", state.project.tracks().len() + 1).into(),
                                gain,
                            ));
                        }
                        audition
                            .new_track_defaults
                            .as_ref()
                            .and_then(|(name, gain)| {
                                hex_session::service::NewTrackMoveDestination::new(
                                    std::sync::Arc::clone(name),
                                    before_track,
                                    *gain,
                                )
                                .map(|destination| {
                                    (
                                        hex_session::service::ArrangementMoveDestination::NewTrack(
                                            destination,
                                        ),
                                        group_position(position),
                                        mode,
                                    )
                                })
                            })
                    } else {
                        None
                    }
                }
                _ => None,
            }
        } else {
            None
        };
        let Some(audition) = self.interaction.clip_move_audition.as_mut() else {
            return false;
        };
        if target
            .as_ref()
            .is_some_and(|(destination, position, mode)| {
                audition.target.as_ref().is_some_and(|last| {
                    &last.destination == destination
                        && (last.position, last.mode) == (*position, *mode)
                })
            })
            || target.is_none() && audition.target.is_none()
        {
            return false;
        }
        let Some((destination, position, mode)) = target else {
            audition.target = None;
            self.expected_clip_preview = None;
            self.applied_clip_preview = None;
            self.service.audition_arrangement_move(None);
            return true;
        };
        let Some(sequence) = audition.sequence.checked_add(1) else {
            self.cancel_held_clip_move_audition();
            return true;
        };
        audition.sequence = sequence;
        let target = hex_session::service::ArrangementMoveAudition {
            context: hex_engine::AudioSourceContext {
                project_generation: drag.project_generation,
                editor_revision: drag.effective_revision(),
            },
            gesture: audition.gesture,
            sequence,
            clips: std::sync::Arc::clone(&audition.clips),
            anchor: drag.clip,
            destination,
            position,
            mode,
        };
        self.expected_clip_preview = Some(target.identity());
        self.clip_preview_may_apply = true;
        self.service.audition_arrangement_move(Some(target.clone()));
        audition.target = Some(target);
        true
    }

    pub(super) fn cancel_held_clip_move_audition(&mut self) {
        if self.interaction.clip_move_audition.take().is_some() {
            self.service.audition_arrangement_move(None);
        }
        self.expected_clip_preview = None;
        self.applied_clip_preview = None;
    }

    fn finish_held_clip_move_audition(&mut self) {
        if self
            .interaction
            .clip_move_audition
            .take()
            .is_some_and(|state| state.target.is_some())
        {
            self.service.finish_arrangement_move_audition();
        }
    }

    /// Revision-advancing requests must settle before `MoveExistingClips` is
    /// dispatched because the worker checks `captured_revision == worker.revision`.
    /// Same-revision worker work (such as tuning audition) does not need deferral.
    pub(super) fn clip_move_must_defer(&self) -> bool {
        self.pending_requests.len() < super::UI_REQUEST_CAPACITY
            && (!self.pending_requests.is_empty()
                || self.duplicate_intent.is_pending()
                || self.sections.pending()
                || self.pending_track_clipboard.is_some()
                || self.track_reorder_active()
                || self.midi_editor.pending_drag.is_some()
                || self.gain_lane.pending.is_some())
    }

    pub(super) fn drop_clip_to_new_track(
        &mut self,
        drag: &ClipDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if drag.group.borrow().ids.len() > 1 {
            self.error =
                Some("clip group move canceled · choose existing destination tracks".into());
            self.clear_clip_drag(drag.project_generation, drag.clip);
            cx.notify();
            return;
        }
        drag.snap_modifier.set(window.modifiers().platform);
        drag.copy_modifier.set(window.modifiers().alt);
        let owns_gesture = self
            .interaction
            .active_clip_drag
            .as_ref()
            .is_some_and(|active| {
                active.project_generation == drag.project_generation
                    && active.clip == drag.clip
                    && Rc::ptr_eq(&active.destination, &drag.destination)
            });
        if !owns_gesture {
            return;
        }
        let current = self.drag_start_is_current(drag);
        let pointer = window.mouse_position();
        let pointer = (f32::from(pointer.x), f32::from(pointer.y));
        drag.last_pointer.set(Some(pointer));
        let (preview, lane_hovered) = self.clip_drag_drop_position(drag, pointer.0, pointer.1);
        drag.preview.set(preview);
        drag.lane_hovered.set(lane_hovered);
        let destination = drag.destination.get();
        if !current {
            self.clear_clip_drag(drag.project_generation, drag.clip);
            return;
        }
        let Some(ClipDropDestination::NewTrack {
            before_track,
            position,
            ..
        }) = destination
        else {
            self.clear_clip_drag(drag.project_generation, drag.clip);
            return;
        };
        if self.edit_blocked_outside_request_queue() || self.pending_clip_move_active() {
            self.error = Some("clip move canceled · project work pending".into());
            self.clear_clip_drag(drag.project_generation, drag.clip);
            cx.notify();
            return;
        }
        self.sync_held_clip_move_audition();
        let defaults = self
            .interaction
            .clip_move_audition
            .as_ref()
            .and_then(|audition| audition.new_track_defaults.clone());
        let (name, gain) = if let Some((name, gain)) = defaults {
            (name.to_string(), gain)
        } else {
            let gain = match self.settings.track_defaults.captured_gain() {
                Ok(gain) => gain,
                Err(error) => {
                    self.error = Some(error.into());
                    self.clear_clip_drag(drag.project_generation, drag.clip);
                    cx.notify();
                    return;
                }
            };
            let count = self
                .state
                .as_ref()
                .map_or(0, |state| state.project.tracks().len());
            (format!("track {}", count + 1), gain)
        };
        let command = if drag.alias_copy() {
            Command::DuplicateAliasClipToNewTrack {
                initial_gain: Some(gain),
                clip: drag.clip,
                name,
                before_track,
                position: group_position(position),
            }
        } else if drag.copy_modifier.get() {
            copy_to_new_track_command(drag.clip, name, before_track, position, gain)
        } else {
            move_to_new_track_command(drag.clip, name, before_track, position, gain)
        };
        let copy_intent = super::group_copy::Pending::capture(self, &command);
        let Some(source) = self
            .state
            .as_ref()
            .map(|state| std::sync::Arc::clone(&state.project))
        else {
            self.clear_clip_drag(drag.project_generation, drag.clip);
            return;
        };
        let defer = self.clip_move_must_defer();
        self.finish_held_clip_move_audition();
        if defer {
            self.pending_clip_move = self.interaction.active_clip_drag.take().map(|drag| {
                super::pending_clip_move::Pending::new_deferred(
                    drag,
                    command.clone(),
                    source,
                    self.interaction.selection_intent_epoch,
                    copy_intent,
                )
            });
            cx.notify();
            return;
        }
        let revision = drag.effective_revision();
        self.pending_clip_move = self.interaction.active_clip_drag.take().map(|drag| {
            super::pending_clip_move::Pending::new(
                drag,
                command.clone(),
                source,
                self.interaction.selection_intent_epoch,
            )
        });
        let before_pending = self.worker_requests_pending;
        if self.enqueue(Request::MoveExistingClips {
            project_generation: drag.project_generation,
            revision,
            command,
        }) {
            if let Some(mut intent) = copy_intent {
                if self.worker_requests_pending > before_pending {
                    intent.submitted(self.worker_requests_pending);
                }
                self.pending_group_copy = Some(intent);
            }
        } else {
            self.retire_pending_clip_move();
            self.clear_clip_drag(drag.project_generation, drag.clip);
        }
        cx.notify();
    }

    pub(super) fn audio_file_drag_payload(
        &self,
        path: std::path::PathBuf,
        label: String,
        browser_generation: Option<u64>,
        metadata: Rc<RefCell<Option<Rc<AudioPreview>>>>,
        metadata_error: Rc<RefCell<Option<String>>>,
    ) -> Option<BrowserAudioDrag> {
        let state = self.state.as_ref()?;
        let viewport = self.visible_viewport(state.timeline_frames);
        Some(BrowserAudioDrag {
            project_generation: self.project_generation,
            revision: state.revision,
            browser_generation,
            sample_rate: state.project.sample_rate(),
            path,
            project_asset: None,
            label,
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: self.snap_grid,
            snap_modifier: Rc::new(Cell::new(false)),
            copy_modifier: Rc::new(Cell::new(false)),
            metadata,
            metadata_error,
            preview: Rc::new(Cell::new(None)),
            destination: Rc::new(Cell::new(None)),
            insertion_line_y: Rc::new(Cell::new(None)),
            lane_hovered: Rc::new(Cell::new(false)),
            last_pointer: Rc::new(Cell::new(None)),
        })
    }

    pub(super) fn begin_browser_audio_drag(&mut self, drag: &BrowserAudioDrag) {
        self.retire_active_clip_drag();
        self.device_panel.active_native_drag = None;
        self.device_panel.sampler_load.header_insertion_line_y = None;
        self.browser.active_audio_drag = Some(drag.clone());
    }

    pub(super) fn begin_native_device_drag(&mut self, drag: &NativeDeviceDrag) {
        self.device_panel.arrangement_drop_preview = None;
        self.retire_active_clip_drag();
        self.browser.active_audio_drag = None;
        if matches!(
            drag.source,
            NativeDeviceDragSource::Browser { .. } | NativeDeviceDragSource::PluginEffect(_)
        ) {
            // Make the native-device receiver visible as soon as a catalog drag
            // becomes active. Keep the selected track/clip untouched: a missing
            // target remains an empty device pane and the eventual drop can refuse.
            self.device_panel.active = true;
            self.audio_detail.clip_view_open = true;
        }
        self.device_panel.active_native_drag = Some(drag.clone());
    }

    pub(super) fn native_device_drag_is_current(&self, drag: &NativeDeviceDrag) -> bool {
        if self.persistence.opening_project || self.persistence.close_prompt_open {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        if self.project_generation != drag.project_generation || state.revision != drag.revision {
            return false;
        }
        match drag.source {
            NativeDeviceDragSource::Browser { .. } | NativeDeviceDragSource::PluginEffect(_) => {
                true
            }
            NativeDeviceDragSource::Existing {
                device,
                class,
                type_id,
            } => {
                state.project.tracks().iter().any(|track| {
                    track.ordered_device_chain().any(|node| {
                        let (candidate, candidate_class) = match node {
                            hex_project::TrackDeviceRef::MidiProcessor(device) => {
                                (device, hex_engine::NativeDeviceClass::MidiProcessor)
                            }
                            hex_project::TrackDeviceRef::Instrument(device) => {
                                (device, hex_engine::NativeDeviceClass::Instrument)
                            }
                            hex_project::TrackDeviceRef::Effect(device) => {
                                (device, hex_engine::NativeDeviceClass::Effect)
                            }
                        };
                        candidate.id() == device
                            && candidate_class == class
                            && candidate.state().type_id() == type_id
                    })
                }) || (class == hex_engine::NativeDeviceClass::Effect
                    && state.project.main_devices().iter().any(|candidate| {
                        candidate.id() == device && candidate.state().type_id() == type_id
                    }))
                    || state.project.return_tracks().iter().any(|track| {
                        class == hex_engine::NativeDeviceClass::Effect
                            && track.devices().iter().any(|candidate| {
                                candidate.id() == device && candidate.state().type_id() == type_id
                            })
                    })
            }
        }
    }

    pub(super) fn clear_native_device_drag(&mut self, drag: &NativeDeviceDrag) {
        if self
            .device_panel
            .active_native_drag
            .as_ref()
            .is_some_and(|active| {
                active.project_generation == drag.project_generation
                    && active.revision == drag.revision
                    && active.source == drag.source
                    && Rc::ptr_eq(&active.destination, &drag.destination)
            })
        {
            self.device_panel.arrangement_drop_preview = None;
            self.device_panel.active_native_drag = None;
            self.device_panel.drag_selection = None;
        }
        drag.destination.set(None);
        drag.return_destination.set(None);
    }

    pub(super) fn cancel_active_native_device_drag(&mut self) -> bool {
        self.device_panel.arrangement_drop_preview = None;
        let Some(drag) = self.device_panel.active_native_drag.take() else {
            return false;
        };
        self.device_panel.drag_selection = None;
        drag.destination.set(None);
        drag.return_destination.set(None);
        true
    }

    pub(super) fn browser_audio_drag_is_current(&self, drag: &BrowserAudioDrag) -> bool {
        if !self
            .browser
            .active_audio_drag
            .as_ref()
            .is_some_and(|active| Rc::ptr_eq(&active.destination, &drag.destination))
            || self.persistence.opening_project
            || self.persistence.close_prompt_open
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || drag
                .browser_generation
                .is_some_and(|generation| self.browser.scan_generation() != generation)
        {
            return false;
        }
        self.state.as_ref().is_some_and(|state| {
            let viewport = self.visible_viewport(state.timeline_frames);
            self.project_generation == drag.project_generation
                && state.revision == drag.revision
                && drag.project_asset.is_none_or(|asset| {
                    self.browser.section == super::browser::BrowserSection::ProjectSamples
                        && state.project.existing_import_frames(asset).is_ok()
                })
                && state.project.sample_rate() == drag.sample_rate
                && viewport.start == drag.viewport().start
                && viewport.length == drag.viewport().length
                && self.snap_grid == drag.snap_grid
        })
    }

    pub(super) fn painted_arrangement_timeline(&self) -> Option<LaneBounds> {
        let body = self.timeline_caches.arrangement_bounds.filter(|_| {
            self.timeline_caches.arrangement_bounds_epoch
                == self.timeline_caches.drag_layout_generation
        })?;
        let timeline = LaneBounds {
            left: body.left,
            width: body.width - self.track_header_width(),
            top: body.top,
            height: body.height,
        };
        (timeline.width > 0.0 && timeline.height > 0.0).then_some(timeline)
    }

    fn painted_arrangement_timeline_at(&self, x: f32, y: f32) -> Option<LaneBounds> {
        self.painted_arrangement_timeline()
            .filter(|&lane| lane_or_leading_buffer_contains_pointer(x, y, lane))
            .filter(|_| !self.painted_arrangement_proposal_at(y))
    }

    fn painted_arrangement_proposal_at(&self, y: f32) -> bool {
        self.arrangement_proposal_rows()
            .filter_map(|row| self.track_scroll.list.bounds_for_item(row))
            .any(|bounds| y >= f32::from(bounds.top()) && y < f32::from(bounds.bottom()))
    }

    pub(super) fn painted_arrangement_header_column(&self) -> Option<LaneBounds> {
        let body = self.timeline_caches.arrangement_bounds.filter(|_| {
            self.timeline_caches.arrangement_bounds_epoch
                == self.timeline_caches.drag_layout_generation
        })?;
        let width = self.track_header_width().min(body.width);
        (width > 0.0 && body.height > 0.0).then_some(LaneBounds {
            left: body.left + body.width - width,
            width,
            ..body
        })
    }

    /// The same vertical insertion gaps as the timeline, restricted to the
    /// scrollable header column. Main and Returns are outside this body.
    pub(super) fn painted_header_track_insertion_target(
        &self,
        pointer_x: f32,
        pointer_y: f32,
    ) -> Option<(Option<TrackId>, LaneBounds, f32)> {
        if self.mode != super::EditorMode::Arrangement {
            return None;
        }
        let headers = self.painted_arrangement_header_column()?;
        if !lane_contains_pointer(pointer_x, pointer_y, headers)
            || self.painted_arrangement_proposal_at(pointer_y)
        {
            return None;
        }
        self.painted_track_insertion_at_y(pointer_y, headers.top)
            .map(|(before, y)| (before, headers, y))
    }

    /// The same vertical insertion gaps as the timeline, restricted to the
    /// scrollable header column. Main and Returns are outside this body.
    pub(super) fn painted_sampler_track_gap(
        &self,
        pointer: gpui::Point<gpui::Pixels>,
    ) -> Option<(Option<TrackId>, f32)> {
        self.painted_header_track_insertion_target(f32::from(pointer.x), f32::from(pointer.y))
            .map(|(before, _, y)| (before, y))
    }

    /// Resolve a pointer inside an expanded automation sub-row's header strip
    /// (below the track's main header, inside its full painted row bounds).
    pub(super) fn hovered_track_automation_header_drop_target(
        &self,
        pointer_x: f32,
        pointer_y: f32,
    ) -> Option<(TrackId, LaneBounds)> {
        if self.mode != super::EditorMode::Arrangement {
            return None;
        }
        let headers = self.painted_arrangement_header_column()?;
        if !lane_contains_pointer(pointer_x, pointer_y, headers)
            || self.painted_arrangement_proposal_at(pointer_y)
        {
            return None;
        }
        self.timeline_caches
            .drag_lanes
            .iter()
            .find_map(|(&track, body_lane)| {
                let full_lane = self.painted_track_drop_lane(track)?;
                let body_bottom = body_lane.top + body_lane.height;
                let full_bottom = full_lane.top + full_lane.height;
                (pointer_y >= body_bottom && pointer_y < full_bottom).then_some((
                    track,
                    LaneBounds {
                        left: headers.left,
                        width: headers.width,
                        top: full_lane.top,
                        height: full_lane.height,
                    },
                ))
            })
    }

    // Use the last painted full row, including attached automation lanes.
    // A virtualized or stale last row cannot claim the trailing hit region.
    fn painted_arrangement_tail_boundary(&self, pointer_y: f32) -> Option<f32> {
        if let Some(row) = self.arrangement_proposal_rows().next_back()
            && row + 1 == self.track_scroll.list.item_count()
        {
            let bottom = f32::from(self.track_scroll.list.bounds_for_item(row)?.bottom());
            return (pointer_y >= bottom).then_some(bottom);
        }
        let state = self.state.as_ref()?;
        let Some(index) = self.arrangement_track_indices().last() else {
            return Some(pointer_y);
        };
        let last = state.project.tracks().get(*index)?;
        let row = self.painted_track_drop_lane(last.id())?;
        let bottom = row.top + row.height;
        (pointer_y >= bottom).then_some(bottom)
    }

    pub(super) fn painted_arrangement_tail(&self, x: f32, y: f32) -> Option<LaneBounds> {
        self.painted_arrangement_tail_bounds()
            .filter(|&lane| lane_or_leading_buffer_contains_pointer(x, y, lane))
    }

    pub(super) fn painted_arrangement_tail_bounds(&self) -> Option<LaneBounds> {
        let timeline = self.painted_arrangement_timeline()?;
        let bottom = timeline.top + timeline.height;
        let top = if self.track_scroll.list.item_count() == 0 {
            timeline.top
        } else {
            self.painted_arrangement_tail_boundary(bottom)?
                .max(timeline.top)
        };
        (top < bottom).then_some(LaneBounds {
            top,
            height: bottom - top,
            ..timeline
        })
    }

    pub(super) fn painted_track_insertion_target(
        &self,
        pointer_x: f32,
        pointer_y: f32,
    ) -> Option<(Option<TrackId>, LaneBounds, f32)> {
        let timeline = self.painted_arrangement_timeline_at(pointer_x, pointer_y)?;
        self.painted_track_insertion_at_y(pointer_y, timeline.top)
            .map(|(before, y)| (before, timeline, y))
    }

    fn painted_track_insertion_at_y(
        &self,
        pointer_y: f32,
        body_top: f32,
    ) -> Option<(Option<TrackId>, f32)> {
        let state = self.state.as_ref()?;
        let tracks = self.arrangement_track_indices();
        if self.track_scroll.list.item_count() == 0 {
            return Some((None, pointer_y));
        }
        if tracks.is_empty() {
            return self
                .painted_arrangement_tail_boundary(pointer_y)
                .map(|bottom| (None, bottom));
        }

        let first = state.project.tracks().get(*tracks.first()?)?;
        if self.timeline_caches.drag_lane_epochs.get(&first.id())
            == Some(&self.timeline_caches.drag_layout_generation)
        {
            let first_lane = self.painted_track_drop_lane(first.id())?;
            // Reserve the same narrow insertion band at the first row's top,
            // without adding empty layout space above the tracks.
            if first_lane.top >= body_top && pointer_y <= first_lane.top + 3.0 {
                return Some((Some(first.id()), first_lane.top));
            }
        }

        for index in 1..tracks.len() {
            let previous = state.project.tracks().get(*tracks.get(index - 1)?)?;
            let next = state.project.tracks().get(*tracks.get(index)?)?;
            if self.timeline_caches.drag_lane_epochs.get(&previous.id())
                != Some(&self.timeline_caches.drag_layout_generation)
                || self.timeline_caches.drag_lane_epochs.get(&next.id())
                    != Some(&self.timeline_caches.drag_layout_generation)
            {
                continue;
            }
            let previous_lane = self.painted_track_drop_lane(previous.id())?;
            let next_lane = self.painted_track_drop_lane(next.id())?;
            let boundary = (previous_lane.top + previous_lane.height + next_lane.top) * 0.5;
            if (pointer_y - boundary).abs() <= 3.0 {
                return Some((Some(next.id()), boundary));
            }
        }

        self.painted_arrangement_tail_boundary(pointer_y)
            .map(|bottom| (None, bottom))
    }

    /// Resolve a captured insertion anchor using only the current painted rows.
    pub(super) fn painted_track_insertion_line(&self, before: Option<TrackId>) -> Option<f32> {
        let state = self.state.as_ref()?;
        let tracks = self.arrangement_track_indices();
        match before {
            Some(id) => {
                let index = tracks.iter().position(|index| {
                    state
                        .project
                        .tracks()
                        .get(*index)
                        .is_some_and(|track| track.id() == id)
                })?;
                let next = self.painted_track_drop_lane(id)?;
                if index == 0 {
                    Some(next.top)
                } else {
                    let previous = state.project.tracks().get(*tracks.get(index - 1)?)?;
                    let previous = self.painted_track_drop_lane(previous.id())?;
                    Some((previous.top + previous.height + next.top) * 0.5)
                }
            }
            None => {
                let last = state.project.tracks().get(*tracks.last()?)?;
                let last = self.painted_track_drop_lane(last.id())?;
                Some(last.top + last.height)
            }
        }
    }

    pub(super) fn browser_audio_drop_position(
        &mut self,
        drag: &BrowserAudioDrag,
        pointer_x: f32,
        pointer_y: f32,
    ) -> (Option<DropPreview>, bool) {
        self.device_panel.sampler_load.header_insertion_line_y = if drag.project_asset.is_none() {
            self.painted_sampler_track_gap(gpui::point(gpui::px(pointer_x), gpui::px(pointer_y)))
                .map(|(_, line)| line)
        } else {
            None
        };
        if !self.browser_audio_drag_is_current(drag) {
            drag.destination.set(None);
            drag.insertion_line_y.set(None);
            return (None, false);
        }
        if self.mode == super::EditorMode::Session {
            // Console receivers own their stable cell/scene target. Browser
            // row moves must not project into retained Arrangement geometry.
            drag.insertion_line_y.set(None);
            return (None, false);
        }
        let Some(state) = self.state.as_ref() else {
            drag.destination.set(None);
            drag.insertion_line_y.set(None);
            return (None, true);
        };
        if let Some((before_track, timeline, line_y)) =
            self.painted_track_insertion_target(pointer_x, pointer_y)
        {
            let Some(start) = browser_start(
                &state.project,
                drag.viewport().start,
                drag.viewport().length,
                drag.snap_grid
                    .with_platform_modifier(drag.snap_modifier.get()),
                pointer_x,
                timeline,
            ) else {
                drag.destination.set(None);
                drag.insertion_line_y.set(None);
                return (None, true);
            };
            drag.destination.set(Some(BrowserDropDestination::NewTrack {
                before_track,
                start,
            }));
            drag.insertion_line_y.set(Some(line_y));
            return (None, true);
        }
        if let Some((target, lane)) = self.hovered_drag_target(pointer_x, pointer_y) {
            let Some(start) = browser_start(
                &state.project,
                drag.viewport().start,
                drag.viewport().length,
                drag.snap_grid
                    .with_platform_modifier(drag.snap_modifier.get()),
                pointer_x,
                lane,
            ) else {
                drag.destination.set(None);
                drag.insertion_line_y.set(None);
                return (None, true);
            };
            drag.destination
                .set(Some(BrowserDropDestination::ExistingTrack {
                    track: target,
                    start,
                }));
            drag.insertion_line_y.set(None);
            let preview = drag
                .metadata
                .borrow()
                .clone()
                .filter(|metadata| metadata.sample_rate == state.project.sample_rate())
                .and_then(|metadata| {
                    browser_candidate(BrowserDropCandidate {
                        project: &state.project,
                        viewport_start: drag.viewport().start,
                        viewport_length: drag.viewport().length,
                        snap_grid: drag.snap_grid,
                        snap_modifier: drag.snap_modifier.get(),
                        pointer_x,
                        lane,
                        duration: metadata.playback_frames,
                        target_track: target,
                    })
                });
            return (preview, true);
        }
        drag.destination.set(None);
        drag.insertion_line_y.set(None);
        (None, false)
    }

    pub(super) fn refresh_browser_audio_drag(&mut self, drag: &BrowserAudioDrag) {
        if !self.browser_audio_drag_is_current(drag) {
            drag.preview.set(None);
            drag.destination.set(None);
            drag.insertion_line_y.set(None);
            drag.lane_hovered.set(false);
            return;
        }
        if self.mode == super::EditorMode::Session {
            if let Some(BrowserDropDestination::Console(target)) = drag.destination.get()
                && !self.console_target_exists(target)
            {
                drag.destination.set(None);
            }
            return;
        }
        let Some((pointer_x, pointer_y)) = drag.last_pointer.get() else {
            return;
        };
        let (preview, lane_hovered) = self.browser_audio_drop_position(drag, pointer_x, pointer_y);
        drag.preview.set(preview);
        drag.lane_hovered.set(lane_hovered);
    }

    pub(super) fn clear_browser_audio_drag(&mut self, drag: &BrowserAudioDrag) {
        if self
            .browser
            .active_audio_drag
            .as_ref()
            .is_some_and(|active| Rc::ptr_eq(&active.destination, &drag.destination))
        {
            self.browser.active_audio_drag = None;
            self.device_panel.sampler_load.header_insertion_line_y = None;
        }
        drag.preview.set(None);
        drag.destination.set(None);
        drag.insertion_line_y.set(None);
        drag.lane_hovered.set(false);
        drag.last_pointer.set(None);
        drag.scroll.set(Default::default());
    }

    pub(super) fn cancel_active_browser_audio_drag(&mut self) -> bool {
        let Some(drag) = self.browser.active_audio_drag.clone() else {
            return false;
        };
        self.clear_browser_audio_drag(&drag);
        true
    }

    pub(super) fn drop_browser_audio(
        &mut self,
        drag: &BrowserAudioDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        drag.snap_modifier.set(window.modifiers().platform);
        drag.copy_modifier.set(window.modifiers().alt);
        let current = self.browser_audio_drag_is_current(drag);
        let (preview, lane_hovered) = self.browser_audio_drop_position(
            drag,
            f32::from(window.mouse_position().x),
            f32::from(window.mouse_position().y),
        );
        drag.preview.set(preview);
        drag.lane_hovered.set(lane_hovered);
        if !current {
            self.browser.active_audio_drag = None;
            cx.notify();
            return;
        }
        let destination = drag.destination.get();
        if destination.is_none() {
            self.browser.active_audio_drag = None;
            cx.notify();
            return;
        }
        let metadata_valid = drag.metadata.borrow().as_ref().is_none_or(|metadata| {
            metadata.sample_rate == drag.sample_rate
                && match destination {
                    Some(BrowserDropDestination::ExistingTrack { .. }) => preview.is_some(),
                    Some(BrowserDropDestination::NewTrack { start, .. }) => {
                        metadata.playback_frames > 0
                            && start.checked_add(metadata.playback_frames).is_some()
                    }
                    Some(BrowserDropDestination::Console(_)) | None => false,
                }
        });
        if !metadata_valid {
            self.browser.active_audio_drag = None;
            self.error = Some("audio drop canceled · preview duration is invalid".into());
            cx.notify();
            return;
        }
        let Some(destination) = destination else {
            self.browser.active_audio_drag = None;
            cx.notify();
            return;
        };
        let request = if let Some(asset) = drag.project_asset {
            let destination = match destination {
                BrowserDropDestination::ExistingTrack { track, start } => {
                    hex_session::import_batch::AudioBatchDestination::Track { track, start }
                }
                BrowserDropDestination::NewTrack {
                    before_track,
                    start,
                } => hex_session::import_batch::AudioBatchDestination::NewTracks {
                    before_track,
                    start,
                },
                BrowserDropDestination::Console(_) => {
                    self.clear_browser_audio_drag(drag);
                    return;
                }
            };
            Request::InsertProjectSample {
                project_generation: drag.project_generation,
                revision: drag.revision,
                asset,
                destination,
                new_track_gain: None,
                warp: hex_project::AudioImportWarp::Off,
                audition_timing: None,
            }
        } else {
            match destination {
                BrowserDropDestination::Console(_) => {
                    self.clear_browser_audio_drag(drag);
                    self.error = Some("sample drop canceled · choose a console cell".into());
                    cx.notify();
                    return;
                }
                BrowserDropDestination::ExistingTrack { track, start } => {
                    Request::ImportToTrackWithOverwrite {
                        path: drag.path.clone(),
                        track,
                        start,
                    }
                }
                BrowserDropDestination::NewTrack {
                    before_track,
                    start,
                } => Request::ImportToNewTrack {
                    path: drag.path.clone(),
                    before_track,
                    start,
                },
            }
        };
        if self.enqueue_sample_drop(request, drag, destination) {
            self.stop_browser_preview_for_drop();
            if self.audio_pub.recovery.deferred_edit.is_none() {
                self.status = "importing sample to arrangement".into();
            }
        }
        self.clear_browser_audio_drag(drag);
        cx.notify();
    }

    pub(super) fn drop_clip(
        &mut self,
        drag: &ClipDrag,
        target_track: TrackId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        drag.snap_modifier.set(window.modifiers().platform);
        drag.copy_modifier.set(window.modifiers().alt);
        if !self
            .interaction
            .active_clip_drag
            .as_ref()
            .is_some_and(|active| Rc::ptr_eq(&active.group, &drag.group))
        {
            return;
        }
        let clear = |view: &mut Self| view.clear_clip_drag(drag.project_generation, drag.clip);
        if self.persistence.opening_project || self.persistence.close_prompt_open {
            clear(self);
            return;
        }
        let pointer = window.mouse_position();
        let pointer = (f32::from(pointer.x), f32::from(pointer.y));
        if self
            .painted_track_insertion_target(pointer.0, pointer.1)
            .is_some()
        {
            if self.edit_blocked_outside_request_queue() || self.pending_clip_move_active() {
                self.error = Some("clip move canceled · project work pending".into());
                clear(self);
                cx.notify();
                return;
            }
            if self.persistence.drag_bounce_held {
                self.error = Some("bounce drop needs an existing track".into());
                clear(self);
                cx.notify();
                return;
            }
            self.drop_clip_to_new_track(drag, window, cx);
            return;
        }
        if self.timeline_caches.arrangement_bounds_epoch
            != self.timeline_caches.drag_layout_generation
            || self.timeline_caches.drag_lane_epochs.get(&target_track)
                != Some(&self.timeline_caches.drag_layout_generation)
        {
            clear(self);
            cx.notify();
            return;
        }
        let Some(state) = self.state.as_ref() else {
            clear(self);
            return;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        if drag.grab_offset.get().is_none() {
            self.error = Some("clip move canceled · pointer position unavailable".into());
            clear(self);
            cx.notify();
            return;
        }
        let Some(lane) = self.timeline_caches.drag_lanes.get(&target_track).copied() else {
            self.error = Some("clip move canceled · target track unavailable".into());
            clear(self);
            cx.notify();
            return;
        };
        let Some(preview) = self.drag_candidate(
            drag,
            target_track,
            f32::from(window.mouse_position().x),
            lane,
        ) else {
            self.error = Some("clip move canceled · invalid timeline position".into());
            clear(self);
            cx.notify();
            return;
        };
        let drop_state = DropState {
            project_generation: self.project_generation,
            revision: state.revision,
            project: &state.project,
            target_track,
            position: preview.position,
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            snap_grid: self.snap_grid,
        };
        if !(if drag.group.borrow().ids.len() > 1 {
            self.drag_start_is_current(drag) && context_is_current(drag, &drop_state)
        } else {
            is_current(drag, drop_state)
        }) {
            self.error = Some("clip move canceled · project changed".into());
            clear(self);
            cx.notify();
            return;
        }
        drag.destination
            .set(Some(ClipDropDestination::ExistingTrack(target_track)));
        if std::mem::take(&mut self.persistence.drag_bounce_held) {
            self.drop_clip_bounce(drag, target_track, preview);
            cx.notify();
            return;
        }
        if is_noop(drag, &drop_state) {
            clear(self);
            return;
        }
        let command = if drag.alias_copy() {
            Command::DuplicateAliasClipsAt {
                clips: drag.group.borrow().ids.clone(),
                anchor: drag.clip,
                track: target_track,
                position: group_position(preview.position),
            }
        } else if drag.group.borrow().ids.len() > 1 {
            Command::MoveClipsWithOverwrite {
                clips: drag.group.borrow().ids.clone(),
                anchor: drag.clip,
                track: target_track,
                position: group_position(preview.position),
                copy: drag.copy_modifier.get(),
            }
        } else if drag.copy_modifier.get() {
            copy_command(drag.clip, target_track, preview.position)
        } else {
            move_command(drag.clip, target_track, preview.position)
        };
        let copy_intent = super::group_copy::Pending::capture(self, &command);
        // Input capture ends here. Keep the frozen destination for moves AND
        // copies until the shared receipt observes this edit's real terminal.
        drag.preview.set(Some(preview));
        let source = std::sync::Arc::clone(&state.project);
        self.sync_held_clip_move_audition();
        let defer = self.clip_move_must_defer();
        self.finish_held_clip_move_audition();
        if defer {
            self.pending_clip_move = self.interaction.active_clip_drag.take().map(|drag| {
                super::pending_clip_move::Pending::new_deferred(
                    drag,
                    command.clone(),
                    source,
                    self.interaction.selection_intent_epoch,
                    copy_intent,
                )
            });
            cx.notify();
            return;
        }
        let revision = drag.effective_revision();
        self.pending_clip_move = self.interaction.active_clip_drag.take().map(|drag| {
            super::pending_clip_move::Pending::new(
                drag,
                command.clone(),
                source,
                self.interaction.selection_intent_epoch,
            )
        });
        let before_pending = self.worker_requests_pending;
        if self.enqueue(Request::MoveExistingClips {
            project_generation: drag.project_generation,
            revision,
            command,
        }) {
            if let Some(mut intent) = copy_intent {
                if self.worker_requests_pending > before_pending {
                    intent.submitted(self.worker_requests_pending);
                }
                self.pending_group_copy = Some(intent);
            }
        } else {
            self.retire_pending_clip_move();
            clear(self);
        }
        cx.notify();
    }
}

impl super::EditorView {
    pub(super) fn refresh_drag_modifiers(
        &mut self,
        held: bool,
        copy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(drag) = self.interaction.active_clip_drag.clone() {
            let snap_changed = drag.snap_modifier.replace(held) != held;
            let copy_changed = drag.copy_modifier.replace(copy) != copy;
            cx.set_active_drag_cursor_style(copy_cursor(copy), window);
            if (snap_changed || copy_changed)
                && let Some((x, y)) = drag.last_pointer.get()
            {
                let (preview, lane_hovered) = self.clip_drag_drop_position(&drag, x, y);
                drag.preview.set(preview);
                drag.lane_hovered.set(lane_hovered);
                cx.notify();
            }
        }
        if let Some(drag) = self.browser.active_audio_drag.clone() {
            let snap_changed = drag.snap_modifier.replace(held) != held;
            let copy_changed = drag.copy_modifier.replace(copy) != copy;
            cx.set_active_drag_cursor_style(copy_cursor(copy), window);
            if (snap_changed || copy_changed)
                && let Some((x, y)) = drag.last_pointer.get()
            {
                let (preview, lane_hovered) = self.browser_audio_drop_position(&drag, x, y);
                drag.preview.set(preview);
                drag.lane_hovered.set(lane_hovered);
                cx.notify();
            }
        }
    }
}

pub(super) fn copy_cursor(copy: bool) -> gpui::CursorStyle {
    if copy {
        gpui::CursorStyle::DragCopy
    } else {
        gpui::CursorStyle::Arrow
    }
}

impl BrowserAudioDrag {
    pub(super) fn viewport(&self) -> Viewport {
        self.scroll.get().viewport.unwrap_or(Viewport {
            start: self.viewport_start,
            length: self.viewport_length,
        })
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "group_drag_ui_test.rs"]
pub(super) mod group_drag_ui_test;
