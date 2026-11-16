//! Main-lane section display and captured, stamped user intents.
//!
//! Project owns section identity, validation and song-wide permutation. This
//! module projects marker geometry and gesture-owned feedback. Release transfers immutable paint to
//! an ordinary FIFO receipt; Escape/blur cancel held input, never admitted work.
//! Option-Command before or during capture latches marker-only intent. Command
//! alone live-inverts snapping; Control remains a snap alias, including marker
//! edits. Option alone copies song content at structural edges.
//! Leaving the lane retains the last valid preview. Body moves/copies use the
//! existing Window pointer lease; its painted cursor follows the held command,
//! while released preview custody remains independent of input capture.
//! Five-pixel inner edges resize one section; a touching seam resizes both, or
//! the left marker's end under Option-Command. Paint and input share native bracket cursors.
//! Shift-click selects the song-order range from the last plain-clicked anchor;
//! Command+Shift-click toggles membership and moves the anchor. Header drags own
//! section edits; ordinary lower-body drags use Main's all-track range owner,
//! including tracks hidden by section focus. A double press in empty lane space
//! paints a section at every height; existing sections retain their input owner.
use super::{
    EditorMode, EditorView, ViewState,
    musical_time::SnapGrid,
    pending_edit::{Disposition, Receipt},
    viewport::Viewport,
};
use crate::{
    context_menu::{MenuItem, PopupMenu},
    section_appearance,
    section_hit::{self, INNER_EDGE_WIDTH, SHARED_EDGE_RADIUS},
    section_input, section_reorder, section_resize, section_selection, theme,
};
use gpui::{
    AnyElement, AppContext, Context, Entity, Subscription, Window, div, prelude::*, px, relative,
    rgb,
};
use hex_project::{ArrangementSection, BeatTime, Command, ElementColor, SectionId, TempoMap};
use hex_session::service::{ArrangementEditViewObservation, Event, Request};
use hex_widgets::{
    RectangleCapture, RectanglePart, RectangleSelection, RectangleSelectionPolicy, TextInput,
};
use std::sync::Arc;

#[cfg(all(test, feature = "ui-test-support"))]
mod beyond_end_tests;
mod clipboard;
#[cfg(all(test, feature = "ui-test-support"))]
mod drop_tests;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SectionEdge {
    Single(SectionId, hex_project::TrimEdge),
    Shared(SectionId, SectionId),
}

#[derive(Clone)]
struct Marker {
    id: Option<SectionId>,
    name: String,
    color: ElementColor,
    start: BeatTime,
    end: BeatTime,
}
impl From<&ArrangementSection> for Marker {
    fn from(s: &ArrangementSection) -> Self {
        Self {
            id: (s.id().value() != 0).then_some(s.id()),
            name: s.name().into(),
            color: s.color(),
            start: s.start(),
            end: s.end(),
        }
    }
}
#[derive(Clone)]
struct Display {
    markers: Vec<Marker>,
    tempo: TempoMap,
    // Owned gesture feedback, frozen with the exact released display. Ordinary
    // hover resumes elsewhere without repainting this receipt's final fill.
    pointer_hover: Option<BeatTime>,
    // Sections under the held edge or body keep one hovered fill for the whole
    // gesture and its receipt; a snapped edge crossing the pointer must not
    // toggle it.
    held: [Option<SectionId>; 2],
    marker_only: bool,
    drop_feedback: Option<DropDisplay>,
}
#[derive(Clone)]
struct DropDisplay {
    feedback: section_reorder::DropFeedback,
    // Earlier FIFO completions may install a different clock while this exact
    // receipt still owns paint. Never reproject its original target with it.
    tempo: TempoMap,
}
impl Display {
    fn pointer_hover_at(
        &self,
        viewport: Viewport,
        [left, top, width, height]: [f32; 4],
        (x, y): (f32, f32),
    ) -> Option<BeatTime> {
        let right = left + width;
        let bottom = top + height;
        if [left, top, width, height, right, bottom, x, y]
            .into_iter()
            .any(|value| !value.is_finite())
            || width <= 0.0
            || height <= 0.0
            || x < left
            || x > right
            || y < top
            || y > bottom
        {
            return None;
        }
        let frame = super::seek::pointer_to_frame(x, left, width, viewport)?;
        self.tempo.frame_to_beat(frame).ok()
    }
}
#[derive(Clone, Copy)]
enum Mode {
    Paint,
    Start(SectionId),
    End(SectionId),
    Move(SectionId),
    Reorder(SectionId),
    Shared {
        left: SectionId,
        right: SectionId,
        marker_only: bool,
    },
    ContentStart(SectionId),
    ContentEnd(SectionId),
}
impl Mode {
    fn marker_only(self) -> bool {
        matches!(
            self,
            Self::Start(_)
                | Self::End(_)
                | Self::Move(_)
                | Self::Shared {
                    marker_only: true,
                    ..
                }
        )
    }

    fn held(self) -> [Option<SectionId>; 2] {
        match self {
            Mode::Paint => [None; 2],
            Mode::Shared { left, right, .. } => [Some(left), Some(right)],
            Mode::Start(id)
            | Mode::End(id)
            | Mode::Move(id)
            | Mode::Reorder(id)
            | Mode::ContentStart(id)
            | Mode::ContentEnd(id) => [Some(id), None],
        }
    }
}
struct Gesture {
    launch_eligible: bool,
    gap_appearance: Option<(String, ElementColor)>,
    generation: u64,
    revision: u64,
    viewport: Viewport,
    snap: SnapGrid,
    layout: [f32; 4],
    anchor: BeatTime,
    last_pointer: (f32, f32),
    capture: RectangleCapture<(u64, Option<SectionId>)>,
    original: Option<Marker>,
    mode: Mode,
    display: Display,
    command: Option<Command>,
}
struct Pending {
    generation: u64,
    revision: u64,
    command: Command,
    display: Display,
    // Projected clip rows at submission, superseded by an exact copy view
    // when available. Retired with the receipt and canonical installation.
    layout: Option<Arc<ViewState>>,
    // The created ID belongs to the exact committed copy observation.
    copied: Option<SectionId>,
    receipt: Receipt,
    selection_intent_epoch: Option<u64>,
}
struct Name {
    id: SectionId,
    generation: u64,
    revision: u64,
    input: Entity<TextInput>,
    _blur: Subscription,
}
/// View-only beat position; a revision change requires a fresh lane click.
#[derive(Clone, Copy)]
struct SectionCursor {
    beat: BeatTime,
    generation: u64,
    revision: u64,
}
#[derive(Default)]
pub(super) struct Sections {
    pub(super) selected: Option<SectionId>,
    selection: Vec<SectionId>,
    // Shift ranges start here; ignored once it leaves the selection.
    anchor: Option<SectionId>,
    cursor: Option<SectionCursor>,
    clipboard: Option<clipboard::Reference>,
    gesture: Option<Gesture>,
    pending: Option<Pending>,
    // At most one keyboard duplicate may follow a committed pending copy.
    ahead: Option<Pending>,
    // One keyboard/menu follow-up typed while an edit is in flight (latest
    // wins). It is validated and submitted against the acknowledged revision.
    queued: Option<Queued>,
    name: Option<Name>,
    launch_focus: std::cell::RefCell<std::collections::BTreeMap<SectionId, gpui::FocusHandle>>,
}
struct Queued {
    generation: u64,
    command: Command,
}
/// Visible refusal for section input that cannot wait for an in-flight edit.
const SECTION_BUSY: &str = "applying section edit · try again when it finishes";
impl Sections {
    pub(super) fn selected_ids(&self) -> &[SectionId] {
        if self.selected.is_some_and(|id| self.selection.contains(&id)) {
            &self.selection
        } else {
            self.selected.as_slice()
        }
    }
    pub(super) fn held(&self) -> bool {
        self.gesture.is_some()
    }
    pub(super) fn pending(&self) -> bool {
        self.pending.is_some()
    }
    pub(super) fn naming(&self) -> bool {
        self.name.is_some()
    }
}
#[derive(Clone)]
pub(super) struct MenuAction {
    generation: u64,
    revision: u64,
    section: SectionId,
    kind: MenuKind,
}
#[derive(Clone)]
enum MenuKind {
    Rename,
    Color(ElementColor),
    Remove,
    Delete,
    Duplicate,
    Copy,
    Paste,
    Merge,
}

impl EditorView {
    fn section_drag_cursor(&self) -> Option<gpui::CursorStyle> {
        let g = self.sections.gesture.as_ref()?;
        if !self.section_current(g) || !matches!(g.mode, Mode::Reorder(_) | Mode::Move(_)) {
            return None;
        }
        Some(
            if g.command.as_ref().and_then(copied_section_source).is_some() {
                super::drag::copy_cursor(true)
            } else if g.capture.displaced(4.0) {
                gpui::CursorStyle::ClosedHand
            } else {
                gpui::CursorStyle::OpenHand
            },
        )
    }
    /// The existing Window lease owns body move/copy input and cursor only.
    /// Release still transfers the original section display to its FIFO receipt.
    pub(super) fn capture_section_drag(&self, window: &mut Window, cx: &Context<Self>) -> bool {
        let Some(cursor) = self.section_drag_cursor() else {
            return true;
        };
        let Some(g) = self.sections.gesture.as_ref() else {
            return true;
        };
        let stamp = (g.generation, g.revision, g.capture.key, g.capture.origin_x);
        let matches = move |g: &Gesture| {
            (g.generation, g.revision, g.capture.key, g.capture.origin_x) == stamp
        };
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        window.capture_pointer(
            cx.entity_id(),
            gpui::MouseButton::Left,
            cursor,
            move |event, window, cx| {
                let moved = event.downcast_ref::<gpui::MouseMoveEvent>();
                let released = event
                    .downcast_ref::<gpui::MouseUpEvent>()
                    .filter(|e| e.button == gpui::MouseButton::Left);
                let Some((position, modifiers)) = moved
                    .map(|e| (e.position, e.modifiers))
                    .or_else(|| released.map(|e| (e.position, e.modifiers)))
                else {
                    return;
                };
                let _ = owner.update(cx, |view, cx| {
                    if !view.sections.gesture.as_ref().is_some_and(matches) {
                        return;
                    }
                    if !view.focus.is_focused(window) {
                        crate::diagnostics::record_section_capture(cx, "focus_cancel");
                        view.cancel_section_pointer();
                    } else if released.is_some() {
                        crate::diagnostics::record_section_capture(cx, "release");
                        view.finish_section_pointer(
                            f32::from(position.x),
                            f32::from(position.y),
                            modifiers,
                        );
                    } else {
                        view.update_section_pointer(
                            f32::from(position.x),
                            f32::from(position.y),
                            modifiers,
                        );
                    }
                    cx.notify();
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.sections.gesture.as_ref().is_some_and(matches) {
                        crate::diagnostics::record_section_capture(cx, "cancel");
                        view.cancel_section_pointer();
                        cx.notify();
                    }
                });
            },
            cx,
        )
    }
    pub(super) fn section_pointer_events(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let entity = cx.entity();
        let empty = self
            .state
            .as_ref()
            .is_some_and(|state| state.project.tracks().is_empty());
        gpui::canvas(
            |_, _, _| (),
            move |_, _, window, cx| {
                let view = entity.read(cx);
                if view.focus.is_focused(window)
                    && let Some(cursor) = view.section_drag_cursor()
                {
                    window.renew_pointer_capture(entity.entity_id(), cursor);
                }
                // Populated arrangements register their shared capture on the
                // existing body canvas; the empty layout has no body capture.
                if !empty {
                    return;
                }
                let down = entity.clone();
                window.on_mouse_event(move |event: &gpui::MouseDownEvent, phase, window, cx| {
                    if !phase.capture() || event.button != gpui::MouseButton::Left {
                        return;
                    }
                    let captured = down.update(cx, |view, cx| {
                        if view.context_menu.is_some()
                            || view.gain_numeric_editing(cx)
                            || view.clip_numeric_editing(cx)
                            || view.project_tempo_editing(cx)
                            || view.pointer_hits_section_launch(
                                f32::from(event.position.x),
                                f32::from(event.position.y),
                            )
                        {
                            return false;
                        }
                        let captured = view.begin_section_pointer(
                            f32::from(event.position.x),
                            f32::from(event.position.y),
                            event.click_count == 2,
                            event.modifiers,
                            cx,
                        );
                        if captured {
                            view.focus.focus(window);
                            if !view.capture_section_drag(window, cx) {
                                view.cancel_section_pointer();
                            }
                            cx.notify();
                        }
                        captured
                    });
                    if captured {
                        cx.stop_propagation();
                    }
                });
                let moved = entity.clone();
                window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                    if !phase.capture() {
                        return;
                    }
                    let changed = moved.update(cx, |view, cx| {
                        let changed = if event.pressed_button == Some(gpui::MouseButton::Left) {
                            view.update_section_pointer(
                                f32::from(event.position.x),
                                f32::from(event.position.y),
                                event.modifiers,
                            )
                        } else {
                            view.cancel_section_pointer()
                        };
                        if changed {
                            cx.notify();
                        }
                        changed
                    });
                    if changed {
                        cx.stop_propagation();
                    }
                });
                let up = entity.clone();
                window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, _, cx| {
                    if !phase.capture() || event.button != gpui::MouseButton::Left {
                        return;
                    }
                    let captured = up.update(cx, |view, cx| {
                        let captured = view.finish_section_pointer(
                            f32::from(event.position.x),
                            f32::from(event.position.y),
                            event.modifiers,
                        );
                        if captured {
                            cx.notify();
                        }
                        captured
                    });
                    if captured {
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
    /// Read the active Main-section selection through the current tempo map.
    /// The outer interval includes gaps. Unknown IDs or an empty frame projection
    /// refuse, so L cannot silently fall back to stale clip/time selection.
    pub(super) fn section_loop_bounds(&self) -> Result<Option<(u64, u64)>, &'static str> {
        if self.mode != EditorMode::Arrangement
            || self.selected_clip.is_some()
            || self.selected_track.is_some()
            || self.focused_return.is_some()
            || self.device_panel.main_selected != Some(self.project_generation)
        {
            return Ok(None);
        }
        let Some(primary) = self.sections.selected else {
            return Ok(None);
        };
        let state = self
            .state
            .as_ref()
            .ok_or("section selection has no current project")?;
        let ids = if self.sections.selection.contains(&primary) {
            self.sections.selection.as_slice()
        } else {
            std::slice::from_ref(&primary)
        };
        let mut start = BeatTime::from_raw(u64::MAX);
        let mut end = BeatTime::ZERO;
        for id in ids {
            let section = state
                .project
                .arrangement_section(*id)
                .ok_or("selected section no longer belongs to the arrangement")?;
            start = start.min(section.start());
            end = end.max(section.end());
        }
        let map = state.project.tempo_map();
        let start = map
            .beat_to_frame(start)
            .map_err(|_| "selected section exceeds the timeline")?;
        let end = map
            .beat_to_frame(end)
            .map_err(|_| "selected section exceeds the timeline")?;
        if start >= end {
            return Err("selected sections have an empty loop range");
        }
        Ok(Some((start, end)))
    }

    /// Arrangement rows display a frozen structural edit or committed copy or paste
    /// until its exact terminal installs the canonical state.
    pub(super) fn arrangement_rows_state(&self) -> Option<Arc<ViewState>> {
        let state = self.state.as_ref()?;
        self.sections
            .ahead
            .as_ref()
            .filter(|p| p.generation == self.project_generation)
            .and_then(|p| p.layout.clone())
            .or_else(|| {
                self.sections
                    .pending
                    .as_ref()
                    .filter(|p| p.generation == self.project_generation)
                    .and_then(|p| p.layout.clone())
            })
            .or_else(|| self.arrangement_duplicate_view().cloned())
            .or_else(|| self.arrangement_paste_view().cloned())
            .or_else(|| self.time_range_copy_view().cloned())
            .or_else(|| Some(Arc::clone(state)))
    }

    /// Replace the projected rows and marker geometry with the exact committed
    /// copy. The ordinary receipt continues to own input, selection and history.
    pub(super) fn observe_section_copy_view(
        &mut self,
        observation: ArrangementEditViewObservation,
        copy: SectionId,
    ) -> bool {
        let Some(current) = self.state.as_ref() else {
            return false;
        };
        let Some(primary) = self.sections.pending.as_ref() else {
            return false;
        };
        let base = if primary.revision == observation.previous_revision {
            Arc::clone(current)
        } else if self
            .sections
            .ahead
            .as_ref()
            .is_some_and(|p| p.revision == observation.previous_revision)
            && current.revision == primary.revision
        {
            let Some(base) = primary
                .layout
                .as_ref()
                .filter(|base| base.revision == observation.previous_revision)
            else {
                return false;
            };
            Arc::clone(base)
        } else {
            return false;
        };
        let pending = if primary.revision == observation.previous_revision {
            self.sections.pending.as_mut()
        } else {
            self.sections.ahead.as_mut()
        };
        let Some(pending) = pending else {
            return false;
        };
        if self.mode != EditorMode::Arrangement
            || self.opening_project
            || pending.receipt.is_queued()
            || pending.generation != self.project_generation
            || observation.project_generation != pending.generation
            || observation.first_previous_revision != observation.previous_revision
            || observation.previous_revision != pending.revision
            || base.revision != pending.revision
            || !(matches!(&pending.command, Command::DuplicateArrangementSection(_))
                || copied_section_source(&pending.command).is_some())
        {
            return false;
        }
        let Some(state) = observation.state.upgrade().filter(|state| {
            pending.revision.checked_add(1) == Some(state.revision)
                && base.project.arrangement_section(copy).is_none()
                && state.project.arrangement_section(copy).is_some()
        }) else {
            return false;
        };
        pending.display.markers = state
            .project
            .arrangement_sections()
            .iter()
            .map(|section| {
                let mut marker = Marker::from(section);
                // New markers keep their preview presentation and remain
                // noninteractive until the terminal installs their IDs.
                if section.id().value() >= current.project.next_identity() {
                    marker.id = None;
                }
                marker
            })
            .collect();
        pending.display.tempo = state.project.tempo_map().clone();
        pending.copied = Some(copy);
        pending.layout = Some(state);
        self.start_ahead_section_duplicate();
        true
    }

    pub(super) fn committed_section_copy_view(&self) -> Option<&Arc<ViewState>> {
        let pending = self.sections.pending.as_ref()?;
        if pending.generation != self.project_generation
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.revision != pending.revision)
        {
            return None;
        }
        if let Some(ahead) = self.sections.ahead.as_ref()
            && ahead.generation == self.project_generation
            && pending.revision.checked_add(1) == Some(ahead.revision)
            && let Some(layout) = ahead
                .layout
                .as_ref()
                .filter(|layout| ahead.revision.checked_add(1) == Some(layout.revision))
        {
            return Some(layout);
        }
        pending
            .layout
            .as_ref()
            .filter(|layout| pending.revision.checked_add(1) == Some(layout.revision))
    }

    /// An early copy start is safe only for the exact installed graph
    /// and its own non-preroll metadata. The ordinary receipt still owns the view.
    pub(super) fn section_copy_play_source(
        &self,
    ) -> Option<hex_audio::prepared::AppliedAudioSource> {
        use std::sync::atomic::Ordering::Acquire;

        let pending = self.sections.pending.as_ref()?;
        let section = match &pending.command {
            Command::DuplicateArrangementSection(section) => *section,
            command => copied_section_source(command)?,
        };
        let current = self.state.as_ref()?;
        let committed = self.committed_section_copy_view()?;
        if self.document_audio_mode != hex_session::DocumentAudioMode::Resident
            || self.selected_clip.is_some()
            || self.sections.selected_ids() != [section]
            || pending.selection_intent_epoch != self.selection_intent_epoch
            || current.project.tempo_map() != committed.project.tempo_map()
            || current.project.loop_region() != committed.project.loop_region()
            || current
                .project
                .arrangement_section(section)
                .map(|s| s.start())
                != committed
                    .project
                    .arrangement_section(section)
                    .map(|s| s.start())
        {
            return None;
        }
        let audio = self.audio.as_ref()?;
        let status = audio.control.status()?;
        if status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
            || status.shutdown_complete.load(Acquire)
            || status.stopped_edit_pending()
        {
            return None;
        }
        let observation = self.pending_publication_observation.as_ref()?;
        let crate::EditorPublicationState::ResidentSubmitted {
            audio_revision,
            metadata,
            ..
        } = &observation.state
        else {
            return None;
        };
        let source = status.recording_source()?;
        if observation.output_identity != source.output_identity
            || audio.control.output_identity() != Some(source.output_identity)
            || observation.context != source.context
            || metadata.context != source.context
            || committed.audio_authoring_revision(source.context) != Some(committed.revision)
            || *audio_revision != source.audio_revision
            || metadata.requires_preroll
            || metadata.clip_preview.is_some()
            || metadata.device_preview.is_some()
            || metadata.tuning_preview.is_some()
            || metadata.seek_request.is_some()
            || metadata.stopped_restore
            || metadata.computer_keyboard_request.is_some()
            || metadata.gain_recording_track.is_some()
            || status.recording_source() != Some(source)
        {
            return None;
        }
        Some(source)
    }

    /// Use the selection the successful terminal will install, without
    /// installing that selection or the committed project prematurely.
    pub(super) fn section_copy_play_start(&self) -> Option<u64> {
        let pending = self.sections.pending.as_ref()?;
        let Some(source) = copied_section_source(&pending.command) else {
            return Some(self.playback_start_frame());
        };
        let selects_copy = self.device_panel.main_selected == Some(self.project_generation)
            && self.sections.selected == Some(source)
            && pending.selection_intent_epoch.is_some()
            && pending.selection_intent_epoch == self.selection_intent_epoch;
        if !selects_copy {
            return Some(self.playback_start_frame());
        }
        let committed = self.committed_section_copy_view()?;
        let copy = committed.project.arrangement_section(pending.copied?)?;
        let frame = committed
            .project
            .tempo_map()
            .beat_to_frame(copy.start())
            .ok()?;
        Some(self.playback_start_frame_with_section_override(Some(frame)))
    }

    pub(super) fn section_install_pending(&self) -> bool {
        self.sections.pending.as_ref().is_some_and(|pending| {
            matches!(
                &pending.command,
                Command::ReorderSectionBefore { .. }
                    | Command::ReorderSectionAfter { .. }
                    | Command::ResizeArrangementSection { .. }
                    | Command::ResizeArrangementSectionEdge { .. }
                    | Command::ResizeArrangementSectionBoundary {
                        marker_only: false,
                        ..
                    }
                    | Command::DeleteArrangementSection(_)
                    | Command::DuplicateArrangementSection(_)
                    | Command::DuplicateArrangementSectionAt { .. }
                    | Command::AppendArrangementSectionCopy { .. }
                    | Command::MoveArrangementSectionBeyondEnd { .. }
                    | Command::ReplaceArrangementSection { .. }
            )
        }) || self
            .audio
            .as_ref()
            .and_then(|audio| audio.control.status())
            .is_some_and(|status| status.stopped_edit_pending())
    }
    fn select_section_metadata(&mut self, id: Option<SectionId>) {
        self.advance_selection_intent();
        self.duplicate_intent.cancel_chain();
        self.sections.selected = id;
        self.sections.selection = id.into_iter().collect();
        self.sections.anchor = id;
        self.selected_track = None;
        self.arrangement_focus.observe_active_track(None);
        self.selected_clip = None;
        self.detail_context = None;
        self.focused_return = None;
        // Use Main's existing view owner so ordinary worker snapshots cannot
        // reselect the first clip and open its inspector under a held preview.
        self.device_panel.main_selected = Some(self.project_generation);
        self.selected_cell = None;
        self.selected_scene = None;
        self.track_selection = Default::default();
        self.time_selection = None;
        self.time_selection_project_scope = false;
        self.cancel_time_selection_gesture();
    }
    pub(super) fn retire_section_pointer_context_for_range(&mut self) {
        self.sections.cursor = None;
        if !self.arrangement_focus.projection.enabled {
            self.sections.selected = None;
            self.sections.selection.clear();
        }
    }

    pub(super) fn section_has_focus(&self, window: &Window) -> bool {
        self.mode == EditorMode::Arrangement
            && self.focus.is_focused(window)
            // Q may retain section IDs while a painted range owns keyboard
            // editing. A stationary Main header point keeps its existing section
            // cursor contract; the range body explicitly retires that cursor.
            && self.time_selection.is_none_or(|selection| {
                selection.anchor_frame == selection.end_frame && self.sections.cursor.is_some()
            })
            && self.selected_clip.is_none()
            && self.selected_track.is_none()
            && self.focused_return.is_none()
            && self.device_panel.main_selected == Some(self.project_generation)
    }
    fn sections_available(&self) -> bool {
        self.sections_idle() && self.section_input_open()
    }
    /// No section or other project edit is in flight.
    fn sections_idle(&self) -> bool {
        !self.edit_pending() && self.sections.pending.is_none() && !self.section_install_pending()
    }
    /// No modal owner, text field or other gesture holds editor input.
    fn section_input_open(&self) -> bool {
        self.mode == EditorMode::Arrangement
            && self.sections.name.is_none()
            && self.clip_name_edit.is_none()
            && !self.track_name.is_editing()
            && self.context_menu.is_none()
            && !super::device_panel::picker_is_open(self)
            && !self.save_dialog_open
            && !self.export_dialog_open
            && !self.project_open.busy()
            && !self.changelog.is_open()
            && !self.tuning.open
            && !self.theme_settings.open
            && self.active_fade_drag.is_none()
            && !self.fade_key_held
            && self.clip_paint_gesture.is_none()
            && self.midi_note_gesture.is_none()
            && self.midi_group_gesture.is_none()
            && self.active_clip_drag.is_none()
            && self.active_trim_drag.is_none()
            && self.active_midi_trim_drag.is_none()
            && self.active_browser_audio_drag.is_none()
            && self.active_native_device_drag.is_none()
            && self.time_selection_gesture.is_none()
            && self.time_range_move_gesture.is_none()
            && self.gain_lane.draft.is_none()
            && self.tuning.client.snapshot().is_none()
            && !self.tuning.client.has_pending_requests()
            && !self.tuning.client.is_recovery_pending()
    }
    fn section_layout(&self) -> Option<[f32; 4]> {
        (self.main_track_lane_epoch == self.drag_layout_generation)
            .then(|| self.section_geometry())
            .flatten()
    }
    // Render advances the layout epoch before Main's paint measurement. Frozen
    // display uses the cached geometry; input separately requires a fresh epoch.
    fn section_geometry(&self) -> Option<[f32; 4]> {
        let lane = self.main_track_lane_bounds?;
        Some([lane.left, lane.top, lane.width, lane.height])
    }
    fn section_pointer(&self, x: f32, y: f32, free: bool) -> Option<BeatTime> {
        let [left, top, width, height] = self.section_layout()?;
        if x < left || x > left + width || y < top || y > top + height {
            return None;
        }
        let state = self.state.as_ref()?;
        let viewport = self.visible_viewport(state.timeline_frames);
        let frame = super::seek::pointer_to_frame(x, left, width, viewport)?;
        self.snap_grid.with_platform_modifier(free).snap_beat(
            &state.project,
            state.project.tempo_map().frame_to_beat(frame).ok()?,
            viewport,
            width,
        )
    }
    fn section_hit(&self, x: f32, y: f32) -> Option<SectionId> {
        let [left, top, width, height] = self.section_layout()?;
        if x < left || x > left + width || y < top || y > top + height {
            return None;
        }
        let state = self.state.as_ref()?;
        let frame = super::seek::pointer_to_frame(
            x,
            left,
            width,
            self.visible_viewport(state.timeline_frames),
        )?;
        let beat = state.project.tempo_map().frame_to_beat(frame).ok()?;
        state
            .project
            .arrangement_sections()
            .iter()
            .find(|s| s.start() <= beat && beat < s.end())
            .map(|s| s.id())
    }
    /// Sections from `anchor` through `clicked` in song order, inclusive.
    fn section_range(&self, anchor: SectionId, clicked: SectionId) -> Vec<SectionId> {
        let Some(project) = self.state.as_ref().map(|state| &state.project) else {
            return vec![clicked];
        };
        crate::section_selection::range(
            project
                .arrangement_sections()
                .iter()
                .map(|section| (section.id(), section.start())),
            anchor,
            clicked,
        )
    }
    fn section_edge_hit(&self, x: f32) -> Option<SectionEdge> {
        self.section_edge_hit_in(x, self.section_layout()?)
    }
    fn section_edge_hit_in(&self, x: f32, geometry: [f32; 4]) -> Option<SectionEdge> {
        let state = self.state.as_ref()?;
        let viewport = self.visible_viewport(state.timeline_frames);
        let [left, _, width, _] = geometry;
        let position = |beat| {
            state
                .project
                .tempo_map()
                .beat_to_frame(beat)
                .ok()
                .map(|frame| {
                    left + (frame as f64 - viewport.start as f64) as f32 / viewport.length as f32
                        * width
                })
        };
        let sections = state
            .project
            .arrangement_sections()
            .iter()
            .map(|section| (section.id(), section.start(), section.end()));
        section_hit::hit(sections, x, position).map(|hit| match hit {
            section_hit::Hit::Leading(id) => SectionEdge::Single(id, hex_project::TrimEdge::Left),
            section_hit::Hit::Trailing(id) => SectionEdge::Single(id, hex_project::TrimEdge::Right),
            section_hit::Hit::Seam(left, right) => SectionEdge::Shared(left, right),
        })
    }

    fn section_current(&self, g: &Gesture) -> bool {
        self.sections_available()
            && g.generation == self.project_generation
            && self.section_geometry() == Some(g.layout)
            && g.snap == self.snap_grid
            && self.state.as_ref().is_some_and(|s| {
                s.revision == g.revision && self.visible_viewport(s.timeline_frames) == g.viewport
            })
    }
    pub(super) fn pointer_hits_section_launch(&self, x: f32, y: f32) -> bool {
        let Some([left, top, width, _]) = self.section_layout() else {
            return false;
        };
        let Some(state) = &self.state else {
            return false;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        state.project.arrangement_sections().iter().any(|section| {
            let tempo = state.project.tempo_map();
            let (Ok(start), Ok(end)) = (
                tempo.beat_to_frame(section.start()),
                tempo.beat_to_frame(section.end()),
            ) else {
                return false;
            };
            let Some((l, r)) = super::arrangement_map::project_interval(
                super::arrangement_map::Interval { start, end },
                viewport,
                width,
            ) else {
                return false;
            };
            super::section_launch::action_bounds(left + l, top, r - l)
                .is_some_and(|bounds| bounds.contains(&gpui::point(px(x), px(y))))
        })
    }

    fn section_launch_action(
        &self,
        id: SectionId,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let generation = self.project_generation;
        let focus = self
            .sections
            .launch_focus
            .borrow_mut()
            .entry(id)
            .or_insert_with(|| cx.focus_handle().tab_index(0).tab_stop(true))
            .clone();
        let click_focus = focus.clone();
        hex_widgets::action(theme::text("▶"), theme::action_style())
            .id(("section-launch", id.value()))
            .debug_selector(move || format!("section-launch-{}", id.value()))
            .absolute()
            .left(px(super::section_launch::ACTION_LEFT))
            .top(px(super::section_launch::ACTION_TOP))
            .size(px(super::section_launch::ACTION_SIZE))
            .rounded_full()
            .text_size(px(8.0))
            .line_height(px(12.0))
            .track_focus(&focus)
            .key_context("hex_section_launch")
            .focus(|style| {
                style
                    .border_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Text)))
            })
            .tooltip(|_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: "play section · hold l to loop".into(),
                })
                .into()
            })
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |view, _, window, cx| {
                cx.stop_propagation();
                if generation == view.project_generation
                    && view.sections_available()
                    && !view.sections.held()
                {
                    click_focus.focus(window);
                    let looped = view.use_section_loop_key();
                    view.launch_section(id, looped);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(
                move |view, _: &super::section_launch::ActivateSectionLaunch, _, cx| {
                    cx.stop_propagation();
                    if generation == view.project_generation
                        && view.sections_available()
                        && !view.sections.held()
                    {
                        view.launch_section(id, false);
                        cx.notify();
                    }
                },
            ))
    }

    pub(super) fn begin_section_pointer(
        &mut self,
        x: f32,
        y: f32,
        double: bool,
        modifiers: gpui::Modifiers,
        cx: &Context<Self>,
    ) -> bool {
        let intent = RectangleSelectionPolicy::Section.intent(modifiers);
        let extend = intent != RectangleSelection::Replace;
        let edit = section_input::EditModifiers::from(modifiers);
        let free = edit.invert_snap;
        let marker_only = edit.marker_only;
        let launch_eligible =
            self.section_autoplay && !double && !extend && !marker_only && !free && !modifiers.alt;
        // This capture also sees clicks on track headers and the focus toggle.
        // Let their focus owners finish before reconciliation decides whether
        // to clear section selection; an armed filter retains its driving IDs.
        if self.section_pointer(x, y, free).is_none() {
            return false;
        }
        if self.sections.naming()
            || self.gain_numeric_editing(cx)
            || self.clip_numeric_editing(cx)
            || self.project_tempo_editing(cx)
        {
            return false;
        }
        let Some(anchor) = self.section_pointer(x, y, free) else {
            return false;
        };
        if !self.sections_available() {
            let hit = self.section_hit(x, y).is_some();
            if hit && !self.sections_idle() {
                self.error = Some(SECTION_BUSY.into());
            }
            return hit;
        }
        let edge = self.section_edge_hit(x);
        let Some([_, top, _, height]) = self.section_layout() else {
            return false;
        };
        let id = match edge {
            Some(SectionEdge::Single(id, _)) | Some(SectionEdge::Shared(id, _)) => Some(id),
            None => self.section_hit(x, y),
        };
        let Some(time_range) = section_input::routes_to_time_range(
            edge.is_some(),
            double && id.is_none(),
            y,
            top,
            height,
            super::clip_name::CLIP_NAME_HEIGHT,
        ) else {
            return false;
        };
        // Section membership takes precedence over the lower-body range owner,
        // edge resizing and autoplay; Shift never creates a time rectangle here.
        if extend && let Some(id) = id {
            let selection = section_selection::extend(
                self.sections.selected_ids().to_vec(),
                self.sections.selected,
                self.sections.anchor,
                id,
                intent == RectangleSelection::Toggle,
                |anchor, clicked| self.section_range(anchor, clicked),
            );
            self.select_section_metadata(Some(id));
            self.sections.selected = selection.selected;
            self.sections.selection = selection.members;
            self.sections.anchor = Some(selection.anchor);
            self.sections.cursor = None;
            self.cancel_section_pointer();
            self.suppress_cursor_click = true;
            return true;
        }
        if time_range {
            // Do not start a competing capture. Main's existing range receiver
            // sees this same press next and keeps canonical all-track scope.
            self.retire_section_pointer_context_for_range();
            return false;
        }
        if !double && self.section_hit(x, y).is_none() && self.main_time_range_grab(x, y).is_some()
        {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let id = match edge {
            Some(SectionEdge::Shared(left, _)) => Some(left),
            _ => self.section_hit(x, y),
        };
        self.sections.cursor = Some(SectionCursor {
            beat: anchor,
            generation: self.project_generation,
            revision: state.revision,
        });
        if id.is_none() && !double {
            if !extend || !self.time_selection_project_scope {
                self.select_section_metadata(None);
            }
            // The Main range receiver also owns empty projects. A stationary
            // point still uses this lane cursor for section shortcuts.
            return false;
        }
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(layout) = self.section_layout() else {
            return false;
        };
        let mut original = id
            .and_then(|id| state.project.arrangement_section(id))
            .map(Marker::from);
        let mode = if let Some(SectionEdge::Shared(left, right)) = edge {
            if marker_only {
                Mode::End(left)
            } else {
                Mode::Shared {
                    left,
                    right,
                    marker_only: false,
                }
            }
        } else if let (Some(_marker), Some(id)) = (&original, id) {
            if edge == Some(SectionEdge::Single(id, hex_project::TrimEdge::Left)) {
                if marker_only {
                    Mode::Start(id)
                } else {
                    Mode::ContentStart(id)
                }
            } else if edge == Some(SectionEdge::Single(id, hex_project::TrimEdge::Right)) {
                if marker_only {
                    Mode::End(id)
                } else {
                    Mode::ContentEnd(id)
                }
            } else if marker_only {
                Mode::Move(id)
            } else {
                Mode::Reorder(id)
            }
        } else {
            Mode::Paint
        };
        if matches!(mode, Mode::Paint) {
            let (name, color) = section_appearance::new_marker(&state.project);
            original = Some(Marker {
                id: None,
                name,
                color,
                start: anchor,
                end: anchor,
            });
        }
        let display = Display {
            markers: state
                .project
                .arrangement_sections()
                .iter()
                .map(Marker::from)
                .collect(),
            tempo: state.project.tempo_map().clone(),
            pointer_hover: None,
            held: [None; 2],
            marker_only: mode.marker_only(),
            drop_feedback: None,
        };
        let part = match edge {
            Some(SectionEdge::Shared(..)) => RectanglePart::Seam,
            Some(SectionEdge::Single(_, edge)) => super::drag::rectangle_part(Some(edge)),
            None => RectanglePart::Body,
        };
        let Some(capture) = RectangleCapture::begin((self.project_generation, id), part, x, true)
        else {
            return false;
        };
        self.sections.gesture = Some(Gesture {
            launch_eligible,
            generation: self.project_generation,
            revision: state.revision,
            viewport,
            snap: self.snap_grid,
            layout,
            anchor,
            last_pointer: (x, y),
            capture,
            original,
            mode,
            display,
            command: None,
            gap_appearance: None,
        });
        self.select_section_metadata(id);
        self.status = if matches!(mode, Mode::Reorder(_)) {
            "section drag · insert or replace song content · option copies · option-command edits markers only · command toggles snap"
        } else if matches!(
            mode,
            Mode::ContentStart(_)
                | Mode::ContentEnd(_)
                | Mode::Shared {
                    marker_only: false,
                    ..
                }
        ) {
            "section resize · crossed time becomes empty · option-command preserves music"
        } else {
            "section marker edit · musical content stays in place"
        }
        .into();
        self.update_section_pointer(x, y, modifiers);
        true
    }
    pub(super) fn update_section_pointer(
        &mut self,
        x: f32,
        y: f32,
        modifiers: gpui::Modifiers,
    ) -> bool {
        let edit = section_input::EditModifiers::from(modifiers);
        let free = edit.invert_snap;
        let Some(mut g) = self.sections.gesture.take() else {
            return false;
        };
        if !self.section_current(&g) || self.section_layout().is_none() {
            return true;
        }
        if edit.marker_only {
            // One-way marker intent prevents releasing Option-Command from unexpectedly
            // turning a preview into a destructive song-content permutation.
            g.mode = match g.mode {
                Mode::Reorder(id) => Mode::Move(id),
                Mode::ContentStart(id) => Mode::Start(id),
                Mode::ContentEnd(id) => Mode::End(id),
                Mode::Shared { left, .. } => Mode::End(left),
                mode => mode,
            };
            g.launch_eligible = false;
        }
        if edit.copy_content || edit.invert_snap {
            g.launch_eligible = false;
        }
        if g.capture.update(x).is_none() {
            self.sections.gesture = Some(g);
            return true;
        }
        let beat = if let Some(beat) = self.section_pointer(x, y, free) {
            g.last_pointer = (x, y);
            beat
        } else if let Some(beat) = self.section_pointer(g.last_pointer.0, g.last_pointer.1, free) {
            // Captured input may finish over the browser or another panel. Keep
            // the last admitted geometry, but still apply live modifier intent.
            beat
        } else {
            self.sections.gesture = Some(g);
            return true;
        };
        g.command = None;
        let Some(state) = self.state.as_ref() else {
            return true;
        };
        g.display = Display {
            markers: state
                .project
                .arrangement_sections()
                .iter()
                .map(Marker::from)
                .collect(),
            tempo: state.project.tempo_map().clone(),
            pointer_hover: None,
            held: [None; 2],
            marker_only: g.mode.marker_only(),
            drop_feedback: None,
        };
        match g.mode {
            Mode::Reorder(id) => {
                // Interior destinations retain stable edges. Beyond the end,
                // the absolute pointer uses the grid. A click never moves music.
                let raw_beat = self
                    .section_pointer(g.last_pointer.0, g.last_pointer.1, true)
                    .unwrap_or(beat);
                if g.capture.displaced(4.0) {
                    match crate::section_hit::drop_edge_frames(g.viewport.length, g.layout[2])
                        .and_then(|edge_frames| {
                            section_reorder::drop_preview(
                                &state.project,
                                id,
                                raw_beat,
                                beat,
                                edit.copy_content,
                                edge_frames,
                                &mut g.gap_appearance,
                            )
                        }) {
                        Ok(Some(preview)) => {
                            g.display.markers =
                                preview.sections().iter().map(Marker::from).collect();
                            g.display.tempo = preview.tempo_map().clone();
                            g.display.drop_feedback =
                                preview.feedback().map(|feedback| DropDisplay {
                                    feedback,
                                    tempo: state.project.tempo_map().clone(),
                                });
                            g.command = Some(preview.command);
                        }
                        Ok(None) => {}
                        Err(error) => self.error = Some(error.to_string().into()),
                    }
                }
            }
            Mode::Move(id) => {
                if g.capture.displaced(4.0)
                    && let Some(marker) = &g.original
                {
                    let delta = i128::from(beat.raw()) - i128::from(g.anchor.raw());
                    let Ok(start) = u64::try_from(i128::from(marker.start.raw()) + delta) else {
                        self.sections.gesture = Some(g);
                        return true;
                    };
                    let start = BeatTime::from_raw(start);
                    if start != marker.start {
                        let command = Command::MoveArrangementSectionMarker { section: id, start };
                        match state.project.preview_arrangement_section_edit(&command) {
                            Ok(preview) => {
                                g.display.markers =
                                    preview.sections().iter().map(Marker::from).collect();
                                g.display.tempo = preview.tempo_map().clone();
                                g.command = Some(command);
                            }
                            Err(error) => self.error = Some(error.to_string().into()),
                        }
                    }
                }
            }
            Mode::Shared {
                left,
                right,
                marker_only,
            } => {
                if g.capture.displaced(4.0) {
                    let edit = section_resize::Edit::Shared {
                        left,
                        right,
                        boundary: beat,
                        marker_only,
                    };
                    match section_resize::preview(&state.project, edit) {
                        Ok(preview) => {
                            g.display.markers =
                                preview.sections().iter().map(Marker::from).collect();
                            g.display.tempo = preview.tempo_map().clone();
                            g.command = Some(preview.command);
                        }
                        Err(error) => self.error = Some(error.to_string().into()),
                    }
                }
            }
            Mode::ContentStart(id) | Mode::ContentEnd(id) => {
                if g.capture.displaced(4.0)
                    && let Some(marker) = &g.original
                {
                    let edge = if matches!(g.mode, Mode::ContentStart(_)) {
                        hex_project::TrimEdge::Left
                    } else {
                        hex_project::TrimEdge::Right
                    };
                    if beat
                        != match edge {
                            hex_project::TrimEdge::Left => marker.start,
                            hex_project::TrimEdge::Right => marker.end,
                        }
                    {
                        let edit = section_resize::Edit::Content {
                            section: id,
                            edge,
                            boundary: beat,
                        };
                        match section_resize::preview(&state.project, edit) {
                            Ok(preview) => {
                                g.display.markers =
                                    preview.sections().iter().map(Marker::from).collect();
                                g.display.tempo = preview.tempo_map().clone();
                                g.command = Some(preview.command);
                            }
                            Err(error) => self.error = Some(error.to_string().into()),
                        }
                    }
                }
            }
            Mode::Start(id) | Mode::End(id) => {
                if g.capture.displaced(4.0)
                    && let Some(marker) = &g.original
                {
                    let edge = if matches!(g.mode, Mode::Start(_)) {
                        hex_project::TrimEdge::Left
                    } else {
                        hex_project::TrimEdge::Right
                    };
                    let original_boundary = if edge == hex_project::TrimEdge::Left {
                        marker.start
                    } else {
                        marker.end
                    };
                    if beat != original_boundary {
                        let edit = section_resize::Edit::Marker {
                            section: id,
                            edge,
                            boundary: beat,
                        };
                        match section_resize::preview(&state.project, edit) {
                            Ok(preview) => {
                                g.display.markers =
                                    preview.sections().iter().map(Marker::from).collect();
                                g.command = Some(preview.command);
                            }
                            Err(error) => self.error = Some(error.to_string().into()),
                        }
                    }
                }
            }
            Mode::Paint => {
                let minimum = self
                    .snap_grid
                    .with_platform_modifier(free)
                    .adjacent_beat(&state.project, g.anchor, g.viewport, g.layout[2], true)
                    .or_else(|| {
                        g.anchor
                            .raw()
                            .checked_add(BeatTime::ONE.raw())
                            .map(BeatTime::from_raw)
                    });
                let Some(minimum) = minimum else {
                    return true;
                };
                let start = beat.min(g.anchor);
                let end = if beat == g.anchor {
                    minimum
                } else {
                    beat.max(g.anchor)
                };
                if start < end
                    && state
                        .project
                        .arrangement_sections()
                        .iter()
                        .all(|s| end <= s.start() || start >= s.end())
                {
                    let Some(appearance) = g.original.as_ref() else {
                        return true;
                    };
                    g.display.markers.push(Marker {
                        id: None,
                        name: appearance.name.clone(),
                        color: appearance.color,
                        start,
                        end,
                    });
                    g.command = Some(Command::AddArrangementSection {
                        name: appearance.name.clone(),
                        color: appearance.color,
                        start,
                        end,
                    });
                }
            }
        }
        g.display.pointer_hover = g
            .display
            .pointer_hover_at(g.viewport, g.layout, g.last_pointer);
        g.display.held = g.mode.held();
        self.sections.gesture = Some(g);
        true
    }
    pub(super) fn finish_section_pointer(
        &mut self,
        x: f32,
        y: f32,
        modifiers: gpui::Modifiers,
    ) -> bool {
        if self.sections.gesture.is_none() {
            return false;
        }
        self.update_section_pointer(x, y, modifiers);
        let Some(g) = self.sections.gesture.take() else {
            return true;
        };
        let Some(_released) = g.capture.finish(x) else {
            return true;
        };
        self.suppress_cursor_click = true;
        if self.section_current(&g) {
            if let Some(command) = g.command {
                self.submit_section(command, g.display, g.generation, g.revision);
            } else if g.launch_eligible
                && !g.capture.displaced(4.0)
                && let Mode::Reorder(id) = g.mode
            {
                self.launch_arrangement_section(id);
            }
        }
        true
    }
    pub(super) fn cancel_section_pointer(&mut self) -> bool {
        self.sections.gesture.take().is_some()
    }
    fn submit_section(
        &mut self,
        command: Command,
        display: Display,
        generation: u64,
        revision: u64,
    ) {
        let layout = self
            .state
            .as_ref()
            .filter(|state| state.revision == revision)
            .and_then(|state| super::section_layout_preview::build(state, &command));
        self.sections.pending = Some(Pending {
            generation,
            revision,
            command: command.clone(),
            display,
            layout,
            copied: None,
            receipt: Receipt::new(generation),
            selection_intent_epoch: self.selection_intent_epoch,
        });
        if !self.enqueue(Request::EditArrangementSection {
            project_generation: generation,
            revision,
            command,
        }) {
            self.sections.pending = None;
        }
    }
    pub(super) fn pending_section_matches(&self, request: &Request) -> bool {
        let Request::EditArrangementSection {
            project_generation,
            revision,
            command,
        } = request
        else {
            return false;
        };
        self.sections
            .pending
            .iter()
            .chain(self.sections.ahead.iter())
            .any(|pending| {
                pending.receipt.is_queued()
                    && *project_generation == pending.generation
                    && *revision == pending.revision
                    && same_command(command, &pending.command)
            })
    }
    pub(super) fn section_submitted(&mut self) {
        if let Some(p) = self
            .sections
            .ahead
            .as_mut()
            .filter(|p| p.receipt.is_queued())
            .or_else(|| {
                self.sections
                    .pending
                    .as_mut()
                    .filter(|p| p.receipt.is_queued())
            })
        {
            p.receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(super) fn retire_pending_section(&mut self) -> bool {
        let pending = self.sections.pending.take().is_some();
        self.sections.ahead.take().is_some() || pending
    }
    pub(super) fn refuse_section(&mut self, request: &Request) -> bool {
        if let Some(ahead) = self.sections.ahead.as_ref()
            && let Request::EditArrangementSection {
                project_generation,
                revision,
                command,
            } = request
            && ahead.receipt.is_queued()
            && *project_generation == ahead.generation
            && *revision == ahead.revision
            && same_command(command, &ahead.command)
        {
            return self.sections.ahead.take().is_some();
        }
        self.retire_pending_section()
    }
    pub(super) fn section_request_complete(&mut self, event: &Event) {
        if self.sections.ahead.as_mut().is_some_and(|ahead| {
            ahead.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            ) == Disposition::Invalidated
        }) {
            self.sections.ahead = None;
        }
        if self.sections.pending.as_mut().is_some_and(|p| {
            p.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            ) != Disposition::Waiting
        }) {
            if event.error.is_none()
                && !event.project_replaced
                && self.device_panel.main_selected == Some(self.project_generation)
                && let Some(Pending {
                    command: Command::MergeArrangementSections(selected),
                    ..
                }) = self.sections.pending.as_ref()
                && self
                    .sections
                    .selected
                    .is_some_and(|id| selected.contains(&id))
                && let Some(kept) = event
                    .state
                    .project
                    .arrangement_sections()
                    .iter()
                    .find(|section| selected.contains(&section.id()))
            {
                self.sections.selected = Some(kept.id());
                self.sections.selection = vec![kept.id()];
            }
            if event.error.is_none()
                && !event.project_replaced
                && let (Some(reference), Some(pending)) = (
                    self.sections.clipboard.as_mut(),
                    self.sections.pending.as_ref(),
                )
            {
                reference.acknowledge_copy(
                    &pending.command,
                    pending.generation,
                    pending.revision,
                    &event.state,
                );
            }
            let copy_source = self.sections.pending.as_ref().and_then(|pending| {
                if pending
                    .selection_intent_epoch
                    .is_none_or(|epoch| Some(epoch) != self.selection_intent_epoch)
                {
                    return None;
                }
                copied_section_source(&pending.command)
            });
            let select_copy = event.error.is_none()
                && !event.project_replaced
                && self.device_panel.main_selected == Some(self.project_generation)
                && copy_source.is_some()
                && copy_source == self.sections.selected;
            if let Some(hex_project::Created::ArrangementSection(id)) =
                event.created.as_ref().filter(|_| select_copy)
            {
                self.sections.selected = Some(*id);
                self.sections.selection = vec![*id];
            }
            if (event.error.is_some() || event.project_replaced)
                && self.sections.queued.take().is_some()
            {
                // It was chosen against the refused preview; never apply it
                // to different content. The terminal error stays visible.
                self.status = "queued section edit dropped".into();
            }
            self.sections.pending = self.sections.ahead.take();
        }
        if event.project_replaced {
            self.sections = Sections::default();
        } else if self
            .sections
            .gesture
            .as_ref()
            .is_some_and(|g| g.revision != event.state.revision)
        {
            self.cancel_section_pointer();
        }
    }
    /// Submit the queued follow-up once its predecessor is acknowledged.
    /// It is validated against the installed revision like a fresh key press.
    pub(super) fn submit_queued_section(&mut self) -> bool {
        if self
            .sections
            .queued
            .as_ref()
            .is_some_and(|q| q.generation != self.project_generation)
        {
            self.sections.queued = None;
            return true;
        }
        if self.sections.queued.is_none() || !self.sections_available() {
            return false;
        }
        let Some(Queued { command, .. }) = self.sections.queued.take() else {
            return false;
        };
        self.status = "applying queued section edit".into();
        self.section_metadata(command);
        true
    }
    /// Undo/Redo order behind the in-flight edit; a queued section follow-up
    /// was aimed at the state they replace, so it is dropped visibly.
    pub(super) fn cancel_queued_section(&mut self) {
        if self.sections.queued.take().is_some() {
            self.status = "queued section edit cancelled by undo/redo".into();
        }
    }
    pub(super) fn reconcile_sections(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(edit) = &self.sections.name {
            super::browser_input::restyle(&edit.input, cx);
        }
        let removed_focus = self
            .sections
            .launch_focus
            .borrow()
            .iter()
            .any(|(id, focus)| {
                focus.is_focused(window)
                    && self
                        .state
                        .as_ref()
                        .is_none_or(|state| state.project.arrangement_section(*id).is_none())
            });
        if removed_focus {
            self.focus.focus(window);
        }
        self.sections.launch_focus.borrow_mut().retain(|id, _| {
            self.state
                .as_ref()
                .is_some_and(|s| s.project.arrangement_section(*id).is_some())
        });
        self.sections.selection.retain(|id| {
            self.state
                .as_ref()
                .is_some_and(|s| s.project.arrangement_section(*id).is_some())
        });

        if self
            .sections
            .gesture
            .as_ref()
            .is_some_and(|g| !self.section_current(g) || self.section_layout().is_none())
        {
            self.cancel_section_pointer();
        }
        if (self.sections.selected.is_some() || self.sections.cursor.is_some())
            && (self.mode != EditorMode::Arrangement
                || self.selected_clip.is_some()
                || self.selected_track.is_some()
                || self.focused_return.is_some()
                || self.device_panel.main_selected != Some(self.project_generation)
                || (!self.focus.is_focused(window)
                    && !self.section_focus_toggle_focused(window, cx)
                    && !self
                        .sections
                        .launch_focus
                        .borrow()
                        .values()
                        .any(|focus| focus.is_focused(window))
                    && !self.sections.naming()
                    && self.context_menu.is_none()))
        {
            if !self.arrangement_focus.projection.enabled {
                self.sections.selected = None;
                self.sections.selection.clear();
            }
            self.sections.cursor = None;
        }
        if self.sections.selected.is_some_and(|id| {
            self.state
                .as_ref()
                .is_none_or(|s| s.project.arrangement_section(id).is_none())
        }) {
            self.sections.selected = None;
            self.sections.selection.clear();
        }
        if self
            .sections
            .name
            .as_ref()
            .is_some_and(|n| !self.section_stamp_current(n.generation, n.revision, n.id))
        {
            let focused = self
                .sections
                .name
                .as_ref()
                .is_some_and(|n| n.input.read(cx).focus_handle().is_focused(window));
            self.sections.name = None;
            if focused {
                self.focus.focus(window);
            }
        }
    }
    fn section_stamp_current(&self, generation: u64, revision: u64, id: SectionId) -> bool {
        generation == self.project_generation
            && self.state.as_ref().is_some_and(|s| {
                s.revision == revision && s.project.arrangement_section(id).is_some()
            })
    }
    pub(super) fn section_name_begin(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.section_has_focus(window) {
            return false;
        }
        let Some(id) = self.sections.selected else {
            return false;
        };
        if !self.sections_available() {
            if !self.sections_idle() {
                self.error = Some(SECTION_BUSY.into());
            }
            return true;
        }
        self.cancel_section_pointer();
        let Some(state) = self.state.as_ref() else {
            return true;
        };
        let Some(section) = state.project.arrangement_section(id) else {
            return true;
        };
        let revision = state.revision;
        let input = cx.new(|cx| super::browser_input::name(section.name().into(), cx));
        let focus = input.read(cx).focus_handle().clone();
        let blur = cx.on_blur(&focus, window, |view, _, cx| {
            view.section_name_commit(cx);
        });
        self.sections.name = Some(Name {
            id,
            generation: self.project_generation,
            revision,
            input,
            _blur: blur,
        });
        focus.focus(window);
        cx.notify();
        true
    }
    fn section_name_commit(&mut self, cx: &mut Context<Self>) {
        let Some(n) = self.sections.name.take() else {
            return;
        };
        let name = n.input.read(cx).text().to_owned();
        if self.section_stamp_current(n.generation, n.revision, n.id) {
            self.section_metadata(Command::RenameArrangementSection {
                section: n.id,
                name,
            });
        }
        cx.notify();
    }
    pub(super) fn finalize_section_name_for_open(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(name) = self.sections.name.as_ref() else {
            return true;
        };
        let draft = name.input.read(cx).text();
        if self.edit_pending()
            || self.project_open.busy()
            || self.pending_open_path.is_some()
            || !self.section_stamp_current(name.generation, name.revision, name.id)
            || draft.trim().is_empty()
            || draft.len() > hex_project::MAX_NAME_BYTES
            || draft.chars().any(char::is_control)
        {
            self.error = Some("finish or cancel the section name before opening a project".into());
            return false;
        }
        self.section_name_finish(true, window, cx);
        true
    }
    pub(super) fn section_name_finish(
        &mut self,
        commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.sections.name.is_none() {
            return false;
        }
        if commit {
            self.section_name_commit(cx);
        } else {
            self.sections.name = None;
        }
        self.focus.focus(window);
        cx.notify();
        true
    }
    pub(super) fn delete_section(&mut self, window: &Window) -> bool {
        self.delete_section_kind(false, window)
    }
    pub(super) fn delete_section_kind(&mut self, marker_only: bool, window: &Window) -> bool {
        let Some(id) = self.sections.selected else {
            return false;
        };
        if !self.section_has_focus(window) {
            return false;
        }
        if self.section_input_open() {
            self.cancel_section_pointer();
            if self.sections.selection.len() > 1 {
                self.error = Some(
                    "select one section to delete, or press g to merge the selected span".into(),
                );
            } else {
                self.section_metadata(if marker_only {
                    Command::RemoveArrangementSection(id)
                } else {
                    Command::DeleteArrangementSection(id)
                });
            }
        }
        true
    }
    pub(super) fn duplicate_section(&mut self, window: &Window) -> bool {
        if !self.section_has_focus(window) {
            return false;
        }
        let Some(id) = self.sections.selected else {
            return false;
        };
        if self.sections.selection.len() > 1 {
            self.error = Some(
                "select one section to duplicate, or press g to merge the selected span".into(),
            );
        } else {
            self.section_metadata(Command::DuplicateArrangementSection(id));
        }
        true
    }
    /// Return true when the lane owns E, including refused/stale points.
    /// Never fall through to clip splitting after a section-context refusal.
    pub(super) fn split_section_at_cursor(&mut self, window: &Window) -> bool {
        if self.sections.naming() {
            return true;
        }
        if !self.section_has_focus(window) || self.normalized_time_selection().is_some() {
            return false;
        }
        let Some(cursor) = self.sections.cursor else {
            return false;
        };
        if !self.sections_available() || self.sections.held() {
            if !self.sections_idle() {
                // The cursor is revision-bound, so a split cannot be queued.
                self.error = Some(SECTION_BUSY.into());
            }
            return true;
        }
        let Some(state) = self.state.as_ref() else {
            return true;
        };
        if cursor.generation != self.project_generation || cursor.revision != state.revision {
            self.error = Some("section cursor changed; click the lane again".into());
            return true;
        }
        let Some(section) = state
            .project
            .arrangement_sections()
            .iter()
            .find(|s| s.start() < cursor.beat && cursor.beat < s.end())
        else {
            self.error = Some("edit cursor must be strictly inside a section to split".into());
            return true;
        };
        self.section_metadata(Command::SplitArrangementSection {
            section: section.id(),
            at: cursor.beat,
        });
        true
    }
    pub(super) fn merge_sections(&mut self, window: &Window) -> bool {
        if !self.section_has_focus(window) || self.sections.selected.is_none() {
            return false;
        }
        self.merge_selected_sections();
        true
    }
    /// Diagnostic projection of the inputs that route `g` (counts and flags only).
    pub(super) fn group_selection_route(&self, window: &Window) -> String {
        format!(
            "mode={:?} focused={} section_focus={} event_focus={} midi_focus={} keyboard_g={} \
             sections={} section_primary={} clips={} clip={} track={} time_range={} \
             section_cursor={} main_selected={} return={} edit_pending={}",
            self.mode,
            self.focus.is_focused(window),
            self.section_has_focus(window),
            self.audio_event_focus.is_focused(window),
            self.midi_focus.is_focused(window),
            self.computer_keyboard.accepts_key("g"),
            self.sections.selection.len(),
            self.sections.selected.is_some(),
            self.selected_clip_ids().len(),
            self.selected_clip.is_some(),
            self.selected_track.is_some(),
            self.time_selection
                .is_some_and(|range| range.anchor_frame != range.end_frame),
            self.sections.cursor.is_some(),
            self.device_panel.main_selected == Some(self.project_generation),
            self.focused_return.is_some(),
            self.edit_pending(),
        )
    }
    /// `g` with a section span as the only grouping selection merges it, even
    /// when the stricter section-focus test fails (Pulse #3812). False when
    /// there is no such span, so the caller explains the empty selection.
    pub(super) fn merge_section_span_selection(&mut self) -> bool {
        if self.sections.selection.len() < 2 || self.selected_clip.is_some() {
            return false;
        }
        self.merge_selected_sections();
        true
    }
    /// `g` on a Main gap creates a marker without cutting the tracks below it.
    /// Occupied ranges keep the existing project-scope grouping route.
    pub(super) fn create_section_in_selected_gap(&mut self) -> bool {
        let (Some(selection), Some(state)) = (self.time_selection, self.state.as_ref()) else {
            return false;
        };
        let start = selection.anchor_frame.min(selection.end_frame);
        let end = selection.anchor_frame.max(selection.end_frame);
        let tempo = state.project.tempo_map();
        let (Ok(start), Ok(end)) = (tempo.frame_to_beat(start), tempo.frame_to_beat(end)) else {
            self.error = Some("section range is outside the timeline".into());
            return true;
        };
        if start >= end {
            self.error = Some("select a longer time range to create a section".into());
            return true;
        }
        if state
            .project
            .arrangement_sections()
            .iter()
            .any(|section| section.start() < end && section.end() > start)
        {
            return false;
        }
        let (name, color) = section_appearance::new_marker(&state.project);
        self.section_metadata(Command::AddArrangementSection {
            name,
            color,
            start,
            end,
        });
        true
    }
    /// Merge the current section selection, or explain why it cannot merge.
    pub(super) fn merge_selected_sections(&mut self) {
        if self.sections.selection.len() < 2 {
            self.error = Some("shift-click another section to merge a span".into());
        } else if !self.section_input_open() {
            self.error = Some("applying edit · wait before merging sections".into());
        } else {
            self.section_metadata(Command::MergeArrangementSections(
                self.sections.selection.clone(),
            ));
        }
    }
    fn section_metadata(&mut self, command: Command) {
        if !self.section_input_open() {
            return;
        }
        if !self.sections_idle() {
            self.sections.queued = Some(Queued {
                generation: self.project_generation,
                command,
            });
            self.status = "section edit queued · applies after the current edit".into();
            self.start_ahead_section_duplicate();
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let mut display = Display {
            markers: state
                .project
                .arrangement_sections()
                .iter()
                .map(Marker::from)
                .collect(),
            tempo: state.project.tempo_map().clone(),
            pointer_hover: None,
            held: [None; 2],
            marker_only: false,
            drop_feedback: None,
        };
        match &command {
            Command::AddArrangementSection {
                name,
                color,
                start,
                end,
            } => {
                let index = display
                    .markers
                    .partition_point(|marker| marker.start < *start);
                display.markers.insert(
                    index,
                    Marker {
                        id: None,
                        name: name.clone(),
                        color: *color,
                        start: *start,
                        end: *end,
                    },
                );
            }
            Command::RenameArrangementSection { section, name } => {
                if let Some(m) = display.markers.iter_mut().find(|m| m.id == Some(*section)) {
                    m.name = name.clone();
                }
            }
            Command::SetArrangementSectionColor { section, color } => {
                if let Some(m) = display.markers.iter_mut().find(|m| m.id == Some(*section)) {
                    m.color = *color;
                }
            }
            Command::RemoveArrangementSection(id) => display.markers.retain(|m| m.id != Some(*id)),
            Command::ResizeArrangementSection { .. }
            | Command::ResizeArrangementSectionEdge { .. }
            | Command::ResizeArrangementSectionBoundary { .. }
            | Command::DeleteArrangementSection(_)
            | Command::DuplicateArrangementSection(_)
            | Command::DuplicateArrangementSectionAt { .. }
            | Command::AppendArrangementSectionCopy { .. }
            | Command::MoveArrangementSectionBeyondEnd { .. }
            | Command::ReplaceArrangementSection { .. }
            | Command::SplitArrangementSection { .. }
            | Command::MergeArrangementSections(_) => {
                match state.project.preview_arrangement_section_edit(&command) {
                    Ok(preview) => {
                        display.markers = preview.sections().iter().map(Marker::from).collect();
                        display.tempo = preview.tempo_map().clone();
                    }
                    Err(error) => {
                        self.error = Some(error.to_string().into());
                        return;
                    }
                }
            }
            _ => return,
        }
        self.submit_section(command, display, self.project_generation, state.revision);
    }

    /// Admit one queued keyboard duplicate only against the exact committed
    /// predecessor. Both FIFO receipts retain their own terminal custody.
    fn start_ahead_section_duplicate(&mut self) {
        if self.sections.ahead.is_some()
            || self.worker_requests_pending != 1
            || !self.pending_requests.is_empty()
            || !self.device_gesture_barriers.is_empty()
            || !self.section_input_open()
        {
            return;
        }
        let Some(primary) = self.sections.pending.as_ref() else {
            return;
        };
        let Command::DuplicateArrangementSection(section) = primary.command else {
            return;
        };
        let Some(queued) = self.sections.queued.as_ref() else {
            return;
        };
        if queued.generation != self.project_generation
            || !matches!(queued.command, Command::DuplicateArrangementSection(id) if id == section)
            || primary.generation != self.project_generation
            || primary.selection_intent_epoch != self.selection_intent_epoch
            || self.sections.selected_ids() != [section]
        {
            return;
        }
        let Some(view) = primary.layout.as_ref().filter(|view| {
            primary.revision.checked_add(1) == Some(view.revision)
                && self
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == primary.revision)
        }) else {
            return;
        };
        let revision = view.revision;
        let command = queued.command.clone();
        self.sections.ahead = Some(Pending {
            generation: self.project_generation,
            revision,
            command: command.clone(),
            display: primary.display.clone(),
            layout: None,
            copied: None,
            receipt: Receipt::new(self.project_generation),
            selection_intent_epoch: primary.selection_intent_epoch,
        });
        self.sections.queued = None;
        if !self.enqueue(Request::EditArrangementSection {
            project_generation: self.project_generation,
            revision,
            command,
        }) {
            self.sections.ahead = None;
        } else {
            self.status = "applying consecutive section copy".into();
        }
    }
    pub(super) fn section_menu(
        &mut self,
        id: SectionId,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sections_available() {
            if !self.sections_idle() {
                self.error = Some(SECTION_BUSY.into());
            }
            return;
        }
        self.cancel_section_pointer();
        let selection = self.sections.selection.clone();
        self.select_section_metadata(Some(id));
        if selection.contains(&id) {
            self.sections.selection = selection;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(section) = state.project.arrangement_section(id) else {
            return;
        };
        let generation = self.project_generation;
        let revision = state.revision;
        let color = section.color();
        let make = |kind| {
            super::context_menu::EditorContextAction::Section(MenuAction {
                generation,
                revision,
                section: id,
                kind,
            })
        };
        let mut items = vec![
            MenuItem {
                label: "duplicate section · cmd-d",
                swatch: None,
                checked: false,
                action: make(MenuKind::Duplicate),
            },
            MenuItem {
                label: "copy section · cmd-c",
                swatch: None,
                checked: false,
                action: make(MenuKind::Copy),
            },
            MenuItem {
                label: "paste section at cursor · cmd-v",
                swatch: None,
                checked: false,
                action: make(MenuKind::Paste),
            },
            MenuItem {
                label: "delete section and close time",
                swatch: None,
                checked: false,
                action: make(MenuKind::Delete),
            },
            MenuItem {
                label: "rename section · cmd-r",
                swatch: None,
                checked: false,
                action: make(MenuKind::Rename),
            },
            MenuItem {
                label: "remove marker only · option-command-delete",
                swatch: None,
                checked: false,
                action: make(MenuKind::Remove),
            },
        ];
        if self.sections.selection.len() >= 2 {
            items.insert(
                0,
                MenuItem {
                    label: "merge selected span · g",
                    swatch: None,
                    checked: false,
                    action: make(MenuKind::Merge),
                },
            );
        }
        items.extend(
            ElementColor::TRACK_CHOICES
                .into_iter()
                .map(|choice| MenuItem {
                    label: choice.key(),
                    swatch: Some(theme::element_colors(choice).accent),
                    checked: choice == color,
                    action: make(MenuKind::Color(choice)),
                }),
        );
        let focus = cx.focus_handle();
        let previous_focus = window.focused(cx);
        focus.focus(window);
        self.context_menu = Some(PopupMenu {
            scroll: gpui::ScrollHandle::new(),
            icon_grid: None,
            position,
            selected: 0,
            focus,
            previous_focus,
            items,
        });
        cx.notify();
    }
    pub(super) fn section_menu_stale(&self, a: &MenuAction) -> bool {
        !self.section_stamp_current(a.generation, a.revision, a.section)
    }
    pub(super) fn section_menu_action(
        &mut self,
        a: MenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.section_menu_stale(&a) {
            return;
        }
        self.sections.selected = Some(a.section);
        match a.kind {
            MenuKind::Rename => {
                self.focus.focus(window);
                self.section_name_begin(window, cx);
            }
            MenuKind::Color(color) => self.section_metadata(Command::SetArrangementSectionColor {
                section: a.section,
                color,
            }),
            MenuKind::Remove => {
                self.focus.focus(window);
                self.delete_section_kind(true, window);
            }
            MenuKind::Delete => {
                self.focus.focus(window);
                self.delete_section_kind(false, window);
            }
            MenuKind::Duplicate => {
                self.focus.focus(window);
                self.duplicate_section(window);
            }
            MenuKind::Copy => {
                self.focus.focus(window);
                self.copy_section(window);
            }
            MenuKind::Paste => {
                self.focus.focus(window);
                self.paste_section(window);
            }
            MenuKind::Merge => {
                self.focus.focus(window);
                self.merge_sections(window);
            }
        }
        cx.notify();
    }
    pub(super) fn section_elements(
        &self,
        viewport: Viewport,
        scale: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(state) = self.state.as_ref() else {
            return Vec::new();
        };
        let canonical;
        let display = if let Some(p) = self
            .sections
            .ahead
            .as_ref()
            .filter(|p| p.layout.is_some())
            .or(self.sections.pending.as_ref())
        {
            &p.display
        } else if let Some(g) = self
            .sections
            .gesture
            .as_ref()
            .filter(|g| self.section_current(g))
        {
            &g.display
        } else {
            canonical = Display {
                markers: state
                    .project
                    .arrangement_sections()
                    .iter()
                    .map(Marker::from)
                    .collect(),
                tempo: state.project.tempo_map().clone(),
                pointer_hover: None,
                held: [None; 2],
                marker_only: false,
                drop_feedback: None,
            };
            &canonical
        };
        let Some(limit) = viewport.end() else {
            return Vec::new();
        };
        if viewport.length == 0 {
            return Vec::new();
        }
        let hover_marker = (self.sections.gesture.is_none()
            && self.sections.pending.is_none()
            && self.sections.name.is_none()
            && section_input::EditModifiers::from(window.modifiers()).marker_only)
            .then(|| {
                let position = window.mouse_position();
                // Rendering advances the epoch before layout measurement. Hover
                // uses cached paint geometry; admitted input still requires freshness.
                let geometry = self.section_geometry()?;
                let beat = display.pointer_hover_at(
                    viewport,
                    geometry,
                    (position.x.into(), position.y.into()),
                )?;
                match self.section_edge_hit_in(position.x.into(), geometry) {
                    Some(SectionEdge::Single(id, _)) | Some(SectionEdge::Shared(id, _)) => Some(id),
                    None => display
                        .markers
                        .iter()
                        .find(|m| m.start <= beat && beat < m.end)
                        .and_then(|m| m.id),
                }
            })
            .flatten();
        let mut elements = Vec::new();
        let mut selected_edges = Vec::new();
        for (index, m) in display.markers.iter().enumerate() {
            let (Ok(start), Ok(end)) = (
                display.tempo.beat_to_frame(m.start),
                display.tempo.beat_to_frame(m.end),
            ) else {
                continue;
            };
            let chamfer = if end <= limit {
                crate::section_surface::CHAMFER
            } else {
                0.0
            };
            let (start, end) = (start.max(viewport.start), end.min(limit));
            if end <= start {
                continue;
            }
            let copy_command = self
                .sections
                .gesture
                .as_ref()
                .and_then(|g| g.command.as_ref())
                .or_else(|| {
                    self.sections
                        .pending
                        .as_ref()
                        .filter(|p| {
                            p.selection_intent_epoch
                                .is_some_and(|epoch| Some(epoch) == self.selection_intent_epoch)
                        })
                        .map(|p| &p.command)
                });
            let copy_preview = copy_command.and_then(copied_section_source).is_some();
            let new_copy = m.id.is_none()
                && copy_command.is_some_and(|command| match command {
                    Command::AppendArrangementSectionCopy { start, .. } => m.start == *start,
                    Command::DuplicateArrangementSectionAt { .. }
                    | Command::ReplaceArrangementSection { copy: true, .. } => true,
                    _ => false,
                });
            let canonical_selected = m.id.is_some_and(|id| {
                self.sections.selection.contains(&id) || self.sections.selected == Some(id)
            });
            let selected = (copy_preview && new_copy) || (!copy_preview && canonical_selected);
            let focused = self.arrangement_focus.projection.enabled && canonical_selected;
            let owned_hover = crate::section_surface::Hover {
                held: display.held,
                pointer: display.pointer_hover,
            }
            .contains(m.id, m.start, m.end);
            let marker_only = (display.marker_only && owned_hover)
                || m.id.is_some_and(|id| hover_marker == Some(id));
            let colors = theme::section_colors(m.color, selected, owned_hover);
            let hover = theme::section_colors(m.color, selected, true);
            let border = colors.2;
            let pixel_width = self.main_track_lane_bounds.map_or(0.0, |lane| {
                lane.width * ((end - start) as f32 / viewport.length as f32)
            });
            let stroke = 1.0 / scale.max(1.0);
            let has_border = hex_widgets::rectangle_border_width(pixel_width) > 0.0;
            let mut element = crate::section_surface::surface(
                (start - viewport.start) as f32 / viewport.length as f32,
                (end - start) as f32 / viewport.length as f32,
                pixel_width, chamfer, m.color, selected, owned_hover,
            )
                .id(("arrangement-section", index))
                .debug_selector(move || format!("arrangement-section-{index}"))
                .top_0()
                .bottom_0()
                .left_0()
                .w_full()
                .tooltip(|_, cx| {
                    cx.new(|_| super::waveform::ClipTooltip { kind: crate::tooltips::Kind::Help,
                        name: "header: select/reorder · body: time selection · inner edge: resize one · seam: resize both · option: copy · option-command: markers only · command: snap · shift-click: select range · cmd-shift-click: toggle · g: merge · cmd-d: duplicate · e: split"
                            .into(),
                    })
                    .into()
                });
            if marker_only {
                element = element.child(
                    crate::section_surface::marker_only_overlay(m.color, chamfer, selected)
                        .debug_selector(move || {
                            format!("arrangement-section-markers-only-{index}")
                        }),
                );
            }
            let mut header = crate::section_surface::header(
                m.color,
                selected,
                owned_hover,
                super::clip_name::CLIP_NAME_HEIGHT,
            )
            .when(marker_only, |header| {
                header.text_color(rgb(theme::color(theme::PaletteRole::Text)))
            })
            .id(("arrangement-section-header", index))
            .debug_selector(move || format!("arrangement-section-header-{index}"));
            if let Some(id) = m.id {
                element = element.on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                        view.section_menu(id, event.position, window, cx);
                        cx.stop_propagation();
                    }),
                );
            }
            let name_edit =
                m.id.and_then(|id| self.sections.name.as_ref().filter(|name| name.id == id));
            if name_edit.is_none() {
                if let Some(id) = m.id.filter(|_| {
                    super::section_launch::action_bounds(0.0, 0.0, pixel_width).is_some()
                }) {
                    header = header
                        .pl(px(16.0))
                        .child(self.section_launch_action(id, cx));
                }
                header = header.child(theme::text(if marker_only {
                    format!("◇ {}", m.name)
                } else {
                    m.name.clone()
                }));
            }
            if focused {
                element =
                    element.children([crate::section_surface::focus::overlay(m.color, chamfer)
                        .debug_selector(move || {
                            format!("arrangement-section-focus-hatch-{index}")
                        })]);
            }
            element = element
                .child(super::arrangement_map::section(self, start, end, chamfer))
                .child(header);
            if let Some(name) = name_edit {
                // Name editing keeps its existing full-size native field; held
                // pointer/range captures are disabled for its entire lifetime.
                element = element.children([name.input.clone()]);
            }
            for right in [false, true] {
                let part = if right {
                    RectanglePart::Trailing
                } else {
                    RectanglePart::Leading
                };
                // Preserve the existing section cursor overlay within its border.
                element =
                    element.children([hex_widgets::rectangle_edge(part, INNER_EDGE_WIDTH, 0.0)]);
            }
            elements.push(
                super::clip_boundary::clip_boundary_shell(
                    element,
                    (start - viewport.start) as f32 / viewport.length as f32,
                    (end - start) as f32 / viewport.length as f32,
                    stroke,
                    border,
                    hover.2,
                    true,
                    has_border && (selected || owned_hover),
                    chamfer,
                    move || format!("arrangement-section-trailing-{index}"),
                )
                .into_any_element(),
            );
            if selected {
                selected_edges.push(
                    div()
                        .debug_selector(move || {
                            format!("arrangement-section-selected-trailing-{index}")
                        })
                        .absolute()
                        .top_0()
                        .bottom(px(chamfer))
                        .left(relative(
                            (end - viewport.start) as f32 / viewport.length as f32,
                        ))
                        .w(px(stroke))
                        .bg(rgb(border))
                        .into_any_element(),
                );
            }
        }
        elements.extend(selected_edges);
        for pair in display.markers.windows(2) {
            if pair[0].end != pair[1].start {
                continue;
            }
            let Ok(frame) = display.tempo.beat_to_frame(pair[0].end) else {
                continue;
            };
            if frame < viewport.start || frame > limit {
                continue;
            }
            elements.push(
                hex_widgets::rectangle_edge(RectanglePart::Seam, 2.0 * SHARED_EDGE_RADIUS, 0.0)
                    .debug_selector(|| "section-shared-edge".into())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(
                        (frame - viewport.start) as f32 / viewport.length as f32,
                    ))
                    .ml(px(-SHARED_EDGE_RADIUS))
                    .into_any_element(),
            );
        }
        if self.sections.gesture.is_none()
            && self.sections.pending.is_none()
            && self.sections.name.is_none()
            && self.normalized_time_selection().is_none()
            && let Some(cursor) = self
                .sections
                .cursor
                .filter(|c| c.generation == self.project_generation && c.revision == state.revision)
            && let Ok(frame) = state.project.tempo_map().beat_to_frame(cursor.beat)
            && let Some(fraction) = viewport.fraction(frame)
        {
            elements.push(
                div()
                    .debug_selector(|| "section-edit-cursor".into())
                    .absolute()
                    .top(px(1.0))
                    .bottom(px(1.0))
                    .left(relative(fraction))
                    .w(px(1.0))
                    .bg(rgb(theme::color(theme::PaletteRole::Text)))
                    .into_any_element(),
            );
        }
        if let Some(drop) = &display.drop_feedback {
            let fraction = |beat| {
                drop.tempo.beat_to_frame(beat).ok().map(|frame| {
                    (frame as f64 - viewport.start as f64) as f32 / viewport.length as f32
                })
            };
            match drop.feedback {
                section_reorder::DropFeedback::Insert(at) => {
                    if let Some(left) = fraction(at) {
                        elements.push(
                            crate::section_surface::insertion_indicator(left)
                                .debug_selector(|| "section-drop-insertion".into())
                                .into_any_element(),
                        );
                    }
                }
                section_reorder::DropFeedback::Replace { start, end, .. } => {
                    if let (Some(left), Some(right)) = (fraction(start), fraction(end)) {
                        elements.push(
                            crate::section_surface::replacement_indicator(left, right - left)
                                .debug_selector(|| "section-drop-replace".into())
                                .into_any_element(),
                        );
                    }
                }
            }
        }
        elements
    }
}
fn copied_section_source(command: &Command) -> Option<SectionId> {
    match command {
        Command::DuplicateArrangementSectionAt { section, .. }
        | Command::AppendArrangementSectionCopy { section, .. } => Some(*section),
        Command::ReplaceArrangementSection {
            source, copy: true, ..
        } => Some(*source),
        _ => None,
    }
}

fn same_command(a: &Command, b: &Command) -> bool {
    match (a, b) {
        (
            Command::ReplaceArrangementSection {
                source: a,
                target: at,
                copy: ac,
            },
            Command::ReplaceArrangementSection {
                source: b,
                target: bt,
                copy: bc,
            },
        ) => (a, at, ac) == (b, bt, bc),
        (
            Command::AppendArrangementSectionCopy {
                section: a,
                start: at,
                gap_name: an,
                gap_color: ac,
            },
            Command::AppendArrangementSectionCopy {
                section: b,
                start: bt,
                gap_name: bn,
                gap_color: bc,
            },
        )
        | (
            Command::MoveArrangementSectionBeyondEnd {
                section: a,
                start: at,
                gap_name: an,
                gap_color: ac,
            },
            Command::MoveArrangementSectionBeyondEnd {
                section: b,
                start: bt,
                gap_name: bn,
                gap_color: bc,
            },
        ) => (a, at, an, ac) == (b, bt, bn, bc),
        (
            Command::DuplicateArrangementSectionAt {
                section: a,
                target: at,
                after: aa,
            },
            Command::DuplicateArrangementSectionAt {
                section: b,
                target: bt,
                after: ba,
            },
        ) => (a, at, aa) == (b, bt, ba),
        (
            Command::AddArrangementSection {
                name: a,
                color: ac,
                start: as_,
                end: ae,
            },
            Command::AddArrangementSection {
                name: b,
                color: bc,
                start: bs,
                end: be,
            },
        ) => (a, ac, as_, ae) == (b, bc, bs, be),
        (
            Command::SetArrangementSectionRange {
                section: a,
                start: as_,
                end: ae,
            },
            Command::SetArrangementSectionRange {
                section: b,
                start: bs,
                end: be,
            },
        ) => (a, as_, ae) == (b, bs, be),
        (
            Command::ResizeArrangementSectionMarker {
                section: a,
                edge: ae,
                boundary: ab,
            },
            Command::ResizeArrangementSectionMarker {
                section: b,
                edge: be,
                boundary: bb,
            },
        ) => (a, ae, ab) == (b, be, bb),
        (
            Command::MoveArrangementSectionMarker {
                section: a,
                start: as_,
            },
            Command::MoveArrangementSectionMarker {
                section: b,
                start: bs,
            },
        ) => (a, as_) == (b, bs),
        (
            Command::ReorderSectionBefore {
                section: a,
                before: at,
            },
            Command::ReorderSectionBefore {
                section: b,
                before: bt,
            },
        ) => (a, at) == (b, bt),
        (
            Command::RenameArrangementSection {
                section: a,
                name: an,
            },
            Command::RenameArrangementSection {
                section: b,
                name: bn,
            },
        ) => (a, an) == (b, bn),
        (
            Command::SetArrangementSectionColor {
                section: a,
                color: ac,
            },
            Command::SetArrangementSectionColor {
                section: b,
                color: bc,
            },
        ) => (a, ac) == (b, bc),
        (
            Command::ReorderSectionAfter {
                section: a,
                after: at,
            },
            Command::ReorderSectionAfter {
                section: b,
                after: bt,
            },
        ) => (a, at) == (b, bt),
        (
            Command::ResizeArrangementSection {
                section: a,
                edge: ae,
                length: al,
            },
            Command::ResizeArrangementSection {
                section: b,
                edge: be,
                length: bl,
            },
        ) => (a, ae, al) == (b, be, bl),
        (
            Command::ResizeArrangementSectionBoundary {
                left: a,
                right: b,
                boundary: c,
                marker_only: d,
            },
            Command::ResizeArrangementSectionBoundary {
                left: x,
                right: y,
                boundary: z,
                marker_only: w,
            },
        ) => (a, b, c, d) == (x, y, z, w),
        (
            Command::ResizeArrangementSectionEdge {
                section: a,
                edge: b,
                boundary: c,
            },
            Command::ResizeArrangementSectionEdge {
                section: x,
                edge: y,
                boundary: z,
            },
        ) => (a, b, c) == (x, y, z),
        (Command::DeleteArrangementSection(a), Command::DeleteArrangementSection(b))
        | (Command::DuplicateArrangementSection(a), Command::DuplicateArrangementSection(b)) => {
            a == b
        }
        (
            Command::SplitArrangementSection { section: a, at: aa },
            Command::SplitArrangementSection { section: b, at: ba },
        ) => (a, aa) == (b, ba),
        (Command::MergeArrangementSections(a), Command::MergeArrangementSections(b)) => a == b,
        (Command::RemoveArrangementSection(a), Command::RemoveArrangementSection(b)) => a == b,
        _ => false,
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
pub(super) mod tests;

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) mod tests {
    use super::*;
    use gpui::{
        Modifiers, MouseButton, MouseDownEvent, TestAppContext, VisualTestContext, point, px,
    };
    use hex_project::Created;
    use hex_session::service::Service;
    use std::time::{Duration, Instant};

    fn painted_fill(visual: &mut VisualTestContext, index: usize) -> Option<gpui::Background> {
        let selector = Box::leak(format!("arrangement-section-{index}").into_boxed_str());
        painted_fill_for(visual, selector)
    }
    fn painted_fill_for(
        visual: &mut VisualTestContext,
        selector: &'static str,
    ) -> Option<gpui::Background> {
        let body = visual.debug_bounds(selector)?;
        let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
        visual
            .rendered_filled_quads(selector)
            .into_iter()
            .filter(|(b, _)| {
                (b.top() / scale - f32::from(body.top())).abs() < 0.1
                    && (b.size.width / scale - f32::from(body.size.width)).abs() < 0.1
                    && b.size.height / scale > 2.0
            })
            .map(|(_, color)| color)
            .next_back()
    }
    #[track_caller]
    pub(in crate::editor) fn next(service: &Service) -> Event {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            if let Ok(event) = service.try_event() {
                return event;
            }
            assert!(Instant::now() < deadline, "service completion");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    pub(in crate::editor) fn fixture(
        cx: &mut TestAppContext,
    ) -> (gpui::AnyWindowHandle, Entity<EditorView>, [SectionId; 2]) {
        cx.update(super::super::bind_editor_keys);
        let service = Service::new(48_000, 32, 8_000_000).unwrap();
        next(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "track".into(),
            }))
            .unwrap();
        let state = next(&service).state;
        let track = state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(16).unwrap(),
            }))
            .unwrap();
        let mut state = next(&service).state;
        let mut ids = Vec::new();
        for (start, end, name, color) in [
            (0, 4, "Intro", ElementColor::Blue),
            (8, 12, "Chorus", ElementColor::Rose),
        ] {
            service
                .submit(Request::EditArrangementSection {
                    project_generation: 0,
                    revision: state.revision,
                    command: Command::AddArrangementSection {
                        name: name.into(),
                        color,
                        start: BeatTime::from_quarters(start).unwrap(),
                        end: BeatTime::from_quarters(end).unwrap(),
                    },
                })
                .unwrap();
            let event = next(&service);
            assert!(event.error.is_none(), "{:?}", event.error);
            let Some(Created::ArrangementSection(id)) = event.created else {
                panic!("section result")
            };
            ids.push(id);
            state = event.state;
        }
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new(window, cx, service, Err("offline section ui test".into()))
                });
                editor.update(cx, |view, cx| {
                    view.state = Some(state);
                    view.focus.focus(window);
                    view.snap_grid = SnapGrid::Quarter;
                    cx.notify();
                });
                editor
            })
            .unwrap();
        (handle, editor, [ids[0], ids[1]])
    }

    #[gpui::test]
    fn g_on_a_main_gap_leaves_underlying_clips_untouched(cx: &mut TestAppContext) {
        use crate::editor::drag::group_drag_ui_test::pump;
        let (handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let lane = visual.debug_bounds("main-track-lane").unwrap();
        let (start, end, before) = editor.read_with(&visual, |view, _| {
            (
                position(view, 5).x,
                position(view, 7).x,
                view.state.as_ref().unwrap().project.clone(),
            )
        });
        let start = point(start, lane.center().y);
        let end = point(end, start.y);
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("g");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            let project = &view.state.as_ref().unwrap().project;
            assert_eq!(
                project.tracks()[0].midi_clips(),
                before.tracks()[0].midi_clips()
            );
            let [intro, section, chorus] = project.arrangement_sections() else {
                panic!("three sections");
            };
            assert_eq!(
                (intro, chorus),
                (
                    &before.arrangement_sections()[0],
                    &before.arrangement_sections()[1]
                )
            );
            assert_eq!(
                (section.start(), section.end()),
                (
                    BeatTime::from_quarters(5).unwrap(),
                    BeatTime::from_quarters(7).unwrap()
                )
            );
        });
    }

    #[gpui::test]
    fn dragging_main_gap_then_g_creates_a_section_even_without_tracks(cx: &mut TestAppContext) {
        use crate::editor::drag::group_drag_ui_test::pump;
        cx.update(super::super::bind_editor_keys);
        let service = Service::new(48_000, 8, 8_000_000).unwrap();
        let state = next(&service).state;
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new(window, cx, service, Err("offline section ui test".into()))
                });
                editor.update(cx, |view, cx| {
                    view.state = Some(state);
                    view.focus.focus(window);
                    view.snap_grid = SnapGrid::Quarter;
                    cx.notify();
                });
                editor
            })
            .unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let lane = visual.debug_bounds("main-track-lane").unwrap();
        let start = point(lane.origin.x + lane.size.width * 0.1, lane.center().y);
        let end = point(lane.origin.x + lane.size.width * 0.4, start.y);
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        let (start, end) = editor.read_with(&visual, |view, _| {
            assert!(view.time_selection_project_scope);
            assert!(view.normalized_time_selection().is_none());
            let selection = view.time_selection.unwrap();
            assert!(selection.anchor_frame < selection.end_frame);
            let map = view.state.as_ref().unwrap().project.tempo_map();
            (
                map.frame_to_beat(selection.anchor_frame).unwrap(),
                map.frame_to_beat(selection.end_frame).unwrap(),
            )
        });
        visual.simulate_keystrokes("g");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            let project = &view.state.as_ref().unwrap().project;
            assert!(project.tracks().is_empty());
            let [section] = project.arrangement_sections() else {
                panic!("created section");
            };
            assert_eq!((section.start(), section.end()), (start, end));
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_sections()
                    .is_empty()
            );
        });
    }
    pub(in crate::editor) fn position(
        view: &EditorView,
        quarters: u64,
    ) -> gpui::Point<gpui::Pixels> {
        let [left, top, width, _] = view.section_layout().unwrap();
        let state = view.state.as_ref().unwrap();
        let viewport = view.visible_viewport(state.timeline_frames);
        let frame = state
            .project
            .tempo_map()
            .beat_to_frame(BeatTime::from_quarters(quarters).unwrap())
            .unwrap();
        point(
            px(left + (frame - viewport.start) as f32 / viewport.length as f32 * width),
            px(top
                + super::super::time_selection::header_hit_height(
                    super::super::tracks::TRACK_ROW_HEIGHT,
                ) * 0.5),
        )
    }
    fn bounds(visual: &mut VisualTestContext, index: usize) -> gpui::Bounds<gpui::Pixels> {
        visual
            .debug_bounds(Box::leak(
                format!("arrangement-section-{index}").into_boxed_str(),
            ))
            .expect("painted section")
    }
    #[gpui::test]
    fn marker_only_paint_tracks_hover_latched_drag_and_exact_ack(cx: &mut TestAppContext) {
        let (handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let (from, to) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 6)));
        let marker = Modifiers {
            platform: true,
            alt: true,
            ..Modifiers::default()
        };
        visual.update(|window, cx| {
            window.dispatch_input(
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position: from,
                    ..Default::default()
                }),
                cx,
            );
        });
        visual.simulate_modifiers_change(marker);
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("arrangement-section-markers-only-0")
                .is_some()
        );
        let hover_fill = painted_fill_for(&mut visual, "arrangement-section-markers-only-0");
        visual.simulate_modifiers_change(Modifiers::default());
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("arrangement-section-markers-only-0")
                .is_none()
        );
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("arrangement-section-markers-only-0")
                .is_none()
        );
        visual.simulate_modifiers_change(marker);
        visual.simulate_modifiers_change(Modifiers::default());
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("arrangement-section-markers-only-0")
                .is_some(),
            "released modifiers retain the actual latched marker-only mode"
        );
        assert_ne!(
            painted_fill_for(&mut visual, "arrangement-section-markers-only-0"),
            hover_fill,
            "selection remains visible within marker-only feedback",
        );
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        let terminal = editor.update(cx, |view, _| next(&view.service));
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert!(
                visual
                    .debug_bounds("arrangement-section-markers-only-0")
                    .is_some()
            );
            assert!(painted_fill_for(&mut visual, "arrangement-section-markers-only-0").is_some());
        }
        editor.update(cx, |view, cx| {
            view.receive(terminal);
            cx.notify();
        });
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("arrangement-section-markers-only-0")
                .is_none()
        );
    }

    #[gpui::test]
    fn option_drag_copies_sections_with_live_modifiers_and_acknowledged_selection(
        cx: &mut TestAppContext,
    ) {
        for (late, select_elsewhere, empty) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let (handle, editor, ids) = fixture(cx);
            if empty {
                editor.update(cx, |view, cx| {
                    let track = view.state.as_ref().unwrap().project.tracks()[0].id();
                    assert!(view.enqueue(Request::Edit(Command::RemoveTrack(track))));
                    let event = next(&view.service);
                    assert!(event.error.is_none());
                    view.receive(event);
                    cx.notify();
                });
            }
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            editor.update(cx, |view, cx| {
                view.viewport =
                    Some(view.visible_viewport(view.state.as_ref().unwrap().timeline_frames));
                view.section_autoplay = true;
                cx.notify();
            });
            visual.run_until_parked();
            let before =
                editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
            let (from, to) =
                editor.read_with(cx, |view, _| (position(view, 2), position(view, 12)));
            let copy = Modifiers {
                alt: true,
                ..Modifiers::default()
            };
            visual.simulate_mouse_down(from, MouseButton::Left, copy);
            visual.simulate_mouse_up(from, MouseButton::Left, copy);
            visual.run_until_parked();
            editor.read_with(cx, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project, before);
                assert!(view.sections.pending.is_none());
                assert!(view.pending_audio_commands.is_empty() && view.prepared_seek.is_none());
            });
            visual.simulate_mouse_down(
                from,
                MouseButton::Left,
                if late { Modifiers::default() } else { copy },
            );
            visual.simulate_mouse_move(
                to,
                MouseButton::Left,
                if late { Modifiers::default() } else { copy },
            );
            visual.simulate_modifiers_change(copy);
            visual.run_until_parked();
            assert_eq!(
                visual.rendered_cursor_style(),
                Some(gpui::CursorStyle::DragCopy)
            );
            editor.read_with(cx, |view, _| {
                assert!(matches!(view.sections.gesture.as_ref().unwrap().command,
                    Some(Command::DuplicateArrangementSectionAt { section, target, after: true })
                    if section == ids[0] && target == ids[1]));
                assert_eq!(view.state.as_ref().unwrap().project, before);
            });
            // Option is live copy intent, unlike latched marker-only Option-Command.
            visual.simulate_modifiers_change(Modifiers::default());
            visual.run_until_parked();
            assert_eq!(
                visual.rendered_cursor_style(),
                Some(gpui::CursorStyle::ClosedHand)
            );
            editor.read_with(cx, |view, _| {
                assert!(matches!(
                    view.sections.gesture.as_ref().unwrap().command,
                    Some(Command::ReorderSectionAfter { .. })
                ))
            });
            visual.simulate_modifiers_change(copy);
            visual.run_until_parked();
            let frozen = [
                bounds(&mut visual, 0),
                bounds(&mut visual, 1),
                bounds(&mut visual, 2),
            ];
            let selected_fill = painted_fill(&mut visual, 2);
            let preview = before
                .preview_arrangement_section_edit(&Command::DuplicateArrangementSectionAt {
                    section: ids[0],
                    target: ids[1],
                    after: true,
                })
                .unwrap();
            let copy_color = preview
                .sections()
                .iter()
                .find(|section| section.id().value() == 0)
                .unwrap()
                .color();
            assert_eq!(
                selected_fill,
                Some(rgb(theme::section_colors(copy_color, true, true).0).into()),
                "the captured copy retains its gesture-owned target feedback"
            );
            visual.simulate_mouse_up(to, MouseButton::Left, copy);
            let terminal = editor.update(cx, |view, _| {
                assert!(view.sections.gesture.is_none());
                assert!(view.sections.pending.is_some());
                next(&view.service)
            });
            assert!(terminal.error.is_none(), "{:?}", terminal.error);
            let Some(Created::ArrangementSection(created)) = terminal.created else {
                panic!("copy identity");
            };
            for _ in 0..3 {
                visual.update(|window, _| window.refresh());
                visual.run_until_parked();
                assert_ne!(
                    visual.rendered_cursor_style(),
                    Some(gpui::CursorStyle::DragCopy),
                    "pending preview must not retain a held copy cursor"
                );
                assert_eq!(
                    [
                        bounds(&mut visual, 0),
                        bounds(&mut visual, 1),
                        bounds(&mut visual, 2)
                    ],
                    frozen
                );
                assert_eq!(painted_fill(&mut visual, 2), selected_fill);
            }
            editor.update(cx, |view, cx| {
                if select_elsewhere {
                    view.select_section_metadata(Some(ids[1]));
                }
                view.receive(terminal);
                assert!(view.sections.pending.is_none());
                let expected = if select_elsewhere { ids[1] } else { created };
                assert_eq!(view.sections.selected, Some(expected));
                assert_eq!(view.sections.selection, vec![expected]);
                let project = &view.state.as_ref().unwrap().project;
                assert_eq!(
                    project.arrangement_section(ids[0]),
                    before.arrangement_section(ids[0])
                );
                let copied = project.arrangement_section(created).unwrap();
                assert_eq!(
                    (copied.start(), copied.end()),
                    (
                        BeatTime::from_quarters(12).unwrap(),
                        BeatTime::from_quarters(16).unwrap()
                    )
                );
                assert!(view.pending_audio_commands.is_empty() && view.prepared_seek.is_none());
                cx.notify();
            });
            visual.run_until_parked();
            assert_eq!(
                [
                    bounds(&mut visual, 0),
                    bounds(&mut visual, 1),
                    bounds(&mut visual, 2)
                ],
                frozen
            );
            if !select_elsewhere {
                // At the exact insertion edge, the pointer is outside the new
                // copy's hover fill after ACK; pending target paint above stays frozen.
                assert_eq!(
                    painted_fill(&mut visual, 2),
                    Some(rgb(theme::section_colors(copy_color, true, false).0).into())
                );
            }
        }
    }

    #[gpui::test]
    fn section_copy_cursor_cancels_without_late_release_edits(cx: &mut TestAppContext) {
        for loss in ["escape", "button", "window", "focus", "generation"] {
            let (handle, editor, _) = fixture(cx);
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.update(|window, _| window.activate_window());
            visual.run_until_parked();
            let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.clone());
            let (from, to) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 13)));
            let copy = Modifiers {
                alt: true,
                ..Modifiers::default()
            };
            visual.simulate_mouse_down(from, MouseButton::Left, copy);
            visual.simulate_mouse_move(to, MouseButton::Left, copy);
            visual.run_until_parked();
            assert_eq!(
                visual.rendered_cursor_style(),
                Some(gpui::CursorStyle::DragCopy)
            );
            let outside = point(px(1.0), px(1.0));
            visual.simulate_mouse_move(outside, MouseButton::Left, copy);
            visual.run_until_parked();
            assert_eq!(
                visual.rendered_cursor_style(),
                Some(gpui::CursorStyle::DragCopy)
            );
            match loss {
                "escape" => visual.simulate_keystrokes("escape"),
                "button" => visual.simulate_mouse_move(outside, None, copy),
                "window" => visual.deactivate_window(),
                "focus" => visual.update(|w, cx| {
                    editor.update(cx, |v, cx| v.search_browser(w, cx));
                }),
                _ => editor.update(cx, |v, cx| {
                    v.project_generation += 1;
                    cx.notify();
                }),
            }
            visual.run_until_parked();
            visual.simulate_mouse_up(outside, MouseButton::Left, copy);
            visual.run_until_parked();
            assert_ne!(
                visual.rendered_cursor_style(),
                Some(gpui::CursorStyle::DragCopy)
            );
            editor.read_with(cx, |v, _| {
                assert!(
                    v.sections.gesture.is_none() && v.sections.pending.is_none(),
                    "{loss}"
                );
                assert_eq!(v.state.as_ref().unwrap().project, before, "{loss}");
                assert_eq!(v.worker_requests_pending, 0);
            });
        }
    }
    #[gpui::test]
    fn option_command_drag_replaces_overlapped_markers_and_retains_preview_until_actual_ack(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(cx, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        let (from, to) = editor.read_with(cx, |view, _| (position(view, 2), position(view, 9)));
        let marker_modifier = Modifiers {
            platform: true,
            alt: true,
            ..Modifiers::default()
        };
        // Start a structural copy, then switch to marker adjustment without
        // moving the pointer. Option-Command replaces the insertion-edge preview.
        let copy = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(from, MouseButton::Left, copy);
        visual.simulate_mouse_move(to, MouseButton::Left, copy);
        // Modifier intent also refreshes while captured outside the lane; an
        // old structural command must never survive the switch to marker mode.
        visual.simulate_mouse_move(point(px(1.0), px(1.0)), MouseButton::Left, copy);
        visual.simulate_modifiers_change(marker_modifier);
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::ClosedHand)
        );
        editor.read_with(cx, |view, _| {
            assert!(matches!(
                view.sections.gesture.as_ref().unwrap().command,
                Some(Command::MoveArrangementSectionMarker { .. })
            ));
            let display = &view.sections.gesture.as_ref().unwrap().display;
            let moved = display
                .markers
                .iter()
                .find(|m| m.id == Some(ids[0]))
                .unwrap();
            let remaining = display
                .markers
                .iter()
                .find(|m| m.id == Some(ids[1]))
                .unwrap();
            assert_eq!(
                (moved.start, moved.end),
                (
                    BeatTime::from_quarters(7).unwrap(),
                    BeatTime::from_quarters(11).unwrap()
                )
            );
            assert_eq!(
                (remaining.start, remaining.end),
                (
                    BeatTime::from_quarters(11).unwrap(),
                    BeatTime::from_quarters(12).unwrap()
                )
            );
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        });
        let preview = [bounds(&mut visual, 0), bounds(&mut visual, 1)];
        visual.simulate_mouse_up(to, MouseButton::Left, marker_modifier);
        let terminal = editor.update(cx, |view, _| {
            let pending = view
                .sections
                .pending
                .as_ref()
                .expect("released marker preview");
            assert!(
                !pending.receipt.is_queued(),
                "the exact move command was admitted"
            );
            next(&view.service)
        });
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
        }
        editor.update(cx, |view, cx| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
            view.receive(terminal);
            assert!(view.sections.pending.is_none());
            let after = view.state.as_ref().unwrap().project.snapshot();
            let mut expected = before.clone();
            expected.arrangement_sections = after.arrangement_sections.clone();
            assert_eq!(after, expected, "only section marker geometry changes");
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
    }

    #[gpui::test]
    fn option_command_resize_live_modifier_carves_both_edges_and_commits_outside_lane(
        cx: &mut TestAppContext,
    ) {
        for (left, late) in [(false, false), (false, true), (true, false), (true, true)] {
            let (handle, editor, ids) = fixture(cx);
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
            let edge = bounds(&mut visual, usize::from(left));
            let down = point(
                if left {
                    edge.left() + px(1.0)
                } else {
                    edge.right() - px(1.0)
                },
                edge.top() + px(12.0),
            );
            let (safe, overlap) = editor.read_with(cx, |v, _| {
                (position(v, 6), position(v, if left { 2 } else { 10 }))
            });
            let marker_modifier = Modifiers {
                platform: true,
                alt: true,
                ..Modifiers::default()
            };
            visual.simulate_mouse_down(
                down,
                MouseButton::Left,
                if late {
                    Modifiers::default()
                } else {
                    marker_modifier
                },
            );
            visual.simulate_mouse_move(
                safe,
                MouseButton::Left,
                if late {
                    Modifiers::default()
                } else {
                    marker_modifier
                },
            );
            if late {
                visual.simulate_modifiers_change(marker_modifier);
            }
            visual.simulate_mouse_move(overlap, MouseButton::Left, marker_modifier);
            visual.run_until_parked();
            editor.read_with(cx, |v, _| {
                assert!(matches!(
                    v.sections.gesture.as_ref().unwrap().command,
                    Some(Command::ResizeArrangementSectionMarker { .. })
                ))
            });
            let preview = [bounds(&mut visual, 0), bounds(&mut visual, 1)];
            let outside = point(px(1.0), px(1.0));
            visual.simulate_mouse_move(outside, MouseButton::Left, marker_modifier);
            visual.simulate_mouse_up(outside, MouseButton::Left, marker_modifier);
            let event = editor.update(cx, |v, _| {
                assert!(
                    v.sections.pending.is_some(),
                    "off-lane release must admit the last valid resize"
                );
                next(&v.service)
            });
            assert!(event.error.is_none(), "{:?}", event.error);
            for _ in 0..3 {
                visual.update(|window, _| window.refresh());
                visual.run_until_parked();
                assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
            }
            editor.update(cx, |v, cx| {
                v.receive(event);
                cx.notify();
            });
            visual.run_until_parked();
            assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
            let after = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
            assert_eq!(
                after.arrangement_sections[0].end,
                BeatTime::from_quarters(if left { 2 } else { 10 }).unwrap()
            );
            assert_eq!(
                after.arrangement_sections[1].start,
                after.arrangement_sections[0].end
            );
            assert_eq!(after.arrangement_sections[0].id, ids[0].value());
            assert_eq!(after.arrangement_sections[1].id, ids[1].value());
            let mut music = after.clone();
            music.arrangement_sections = before.arrangement_sections.clone();
            assert_eq!(music, before);
            for (request, expected) in [(Request::Undo, &before), (Request::Redo, &after)] {
                editor.update(cx, |v, cx| {
                    v.service.submit(request).unwrap();
                    let event = next(&v.service);
                    assert!(event.error.is_none());
                    v.receive(event);
                    assert_eq!(&v.state.as_ref().unwrap().project.snapshot(), expected);
                    cx.notify();
                });
                visual.run_until_parked();
            }
        }
    }

    #[gpui::test]
    fn option_command_move_latches_marker_mode_and_preserves_control_snap_inversion(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(cx, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        for free in [false, true] {
            let (from, to) = editor.read_with(cx, |view, _| {
                let a = position(view, 6);
                let b = position(view, 7);
                (position(view, 2), point(a.x + (b.x - a.x) * 0.37, a.y))
            });
            visual.simulate_mouse_down(
                from,
                MouseButton::Left,
                Modifiers {
                    platform: true,
                    alt: true,
                    ..Modifiers::default()
                },
            );
            // Releasing Option-Command must not turn this into a music edit.
            visual.simulate_mouse_move(
                to,
                MouseButton::Left,
                Modifiers {
                    control: free,
                    ..Modifiers::default()
                },
            );
            editor.read_with(cx, |view, _| {
                let Some(Command::MoveArrangementSectionMarker { section, start }) =
                    &view.sections.gesture.as_ref().unwrap().command
                else {
                    panic!("expected latched marker move")
                };
                assert_eq!(*section, ids[0]);
                let four = BeatTime::from_quarters(4).unwrap();
                if free {
                    assert!(*start > four && *start < BeatTime::from_quarters(5).unwrap());
                } else {
                    assert_eq!(*start, four);
                }
            });
            visual.simulate_keystrokes("escape");
            visual.run_until_parked();
            editor.read_with(cx, |view, _| {
                assert!(view.sections.gesture.is_none());
                assert!(view.sections.pending.is_none());
                assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
    }

    #[gpui::test]
    fn paint_empty_section_lane_at_every_height(cx: &mut TestAppContext) {
        for (fraction, reverse) in [(0.1, false), (0.5, true), (0.9, false)] {
            let (handle, editor, _) = fixture(cx);
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
            let (from, to) = editor.read_with(cx, |v, _| {
                let [_, top, _, height] = v.section_layout().unwrap();
                let mut from = position(v, if reverse { 7 } else { 5 });
                let mut to = position(v, if reverse { 5 } else { 7 });
                from.y = px(top + height * fraction);
                to.y = from.y;
                (from, to)
            });
            visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(from, MouseButton::Left, Modifiers::default());
            visual.simulate_event(MouseDownEvent {
                position: from,
                button: MouseButton::Left,
                click_count: 2,
                modifiers: Modifiers::default(),
                first_mouse: false,
            });
            visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(cx, |v, _| {
                assert!(
                    matches!(v.sections.gesture.as_ref().and_then(|g| g.command.as_ref()),
                    Some(Command::AddArrangementSection { start, end, .. })
                    if *start == BeatTime::from_quarters(5).unwrap()
                        && *end == BeatTime::from_quarters(7).unwrap()),
                    "height {fraction}"
                );
                assert!(v.time_selection_gesture.is_none());
            });
            let preview = bounds(&mut visual, 2);
            visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
            let terminal = editor.update(cx, |v, _| next(&v.service));
            assert!(terminal.error.is_none(), "{:?}", terminal.error);
            for _ in 0..2 {
                visual.update(|window, _| window.refresh());
                visual.run_until_parked();
                assert_eq!(bounds(&mut visual, 2), preview);
            }
            editor.update(cx, |v, cx| {
                v.receive(terminal);
                assert_eq!(
                    v.state
                        .as_ref()
                        .unwrap()
                        .project
                        .arrangement_sections()
                        .len(),
                    3
                );
                v.service.submit(Request::Undo).unwrap();
                let terminal = next(&v.service);
                assert!(terminal.error.is_none());
                v.receive(terminal);
                let restored = v.state.as_ref().unwrap().project.snapshot();
                let mut expected = before.clone();
                assert!(restored.next_id >= expected.next_id);
                expected.next_id = restored.next_id;
                assert_eq!(restored, expected, "one undo restores all content");
                cx.notify();
            });
            visual.run_until_parked();
            let first = bounds(&mut visual, 0);
            let occupied = point(first.center().x, first.top() + first.size.height * fraction);
            visual.simulate_event(MouseDownEvent {
                position: occupied,
                button: MouseButton::Left,
                click_count: 2,
                modifiers: Modifiers::default(),
                first_mouse: false,
            });
            editor.read_with(cx, |v, _| {
                if fraction < 0.3 {
                    assert!(matches!(
                        v.sections.gesture.as_ref().map(|g| g.mode),
                        Some(Mode::Reorder(_))
                    ));
                } else {
                    assert!(v.sections.gesture.is_none());
                    assert!(v.time_selection_gesture.is_some());
                }
            });
            visual.simulate_keystrokes("escape");
        }
    }

    #[gpui::test]
    fn paint_and_resize_main_sections_preview_without_project_writes(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        assert!(visual.debug_bounds("main-track-lane").is_some());
        let (start, end) = editor.read_with(cx, |v, _| (position(v, 5), position(v, 7)));
        let original = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.clone());
        visual.simulate_event(MouseDownEvent {
            position: start,
            button: MouseButton::Left,
            click_count: 2,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(bounds(&mut visual, 2).size.width > px(1.0));
        editor.read_with(cx, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project, original);
            assert!(matches!(
                v.sections.gesture.as_ref().unwrap().command,
                Some(Command::AddArrangementSection { .. })
            ));
            assert!(v.time_selection_gesture.is_none());
        });
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert!(visual.debug_bounds("arrangement-section-2").is_none());
        // The right edge edits marker geometry only, preserving every track.
        let edge = bounds(&mut visual, 0);
        let down = point(
            edge.origin.x + edge.size.width - px(1.0),
            edge.origin.y + px(12.0),
        );
        visual.simulate_mouse_down(
            down,
            MouseButton::Left,
            Modifiers {
                platform: true,
                alt: true,
                ..Modifiers::default()
            },
        );
        let target = editor.read_with(cx, |v, _| position(v, 6));
        visual.simulate_mouse_move(
            target,
            MouseButton::Left,
            Modifiers {
                platform: true,
                alt: true,
                ..Modifiers::default()
            },
        );
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(matches!(
                v.sections.gesture.as_ref().unwrap().command,
                Some(Command::ResizeArrangementSectionMarker { section, boundary: end, .. })
                    if section == ids[0] && end == BeatTime::from_quarters(6).unwrap()
            ));
            assert_eq!(v.state.as_ref().unwrap().project, original);
        });
        assert!(bounds(&mut visual, 0).size.width > edge.size.width);
        // Revision/viewport changes invalidate a held operation before release.
        editor.update(cx, |v, cx| {
            v.snap_grid = SnapGrid::Off;
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(cx, |v, _| assert!(!v.sections.held()));
        assert_eq!(bounds(&mut visual, 0).size.width, edge.size.width);
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        editor.update(cx, |v, _| v.snap_grid = SnapGrid::Quarter);
        let (anchor, destination) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 3)));
        let alt = Modifiers {
            platform: true,
            alt: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(anchor, MouseButton::Left, alt);
        visual.simulate_mouse_move(destination, MouseButton::Left, alt);
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(matches!(
                v.sections.gesture.as_ref().unwrap().command,
                Some(Command::MoveArrangementSectionMarker { section, start })
                    if section == ids[0] && start == BeatTime::ONE
            ));
            assert_eq!(v.state.as_ref().unwrap().project, original);
        });
        assert!(visual.debug_bounds("section-insertion-preview").is_none());
        visual.simulate_keystrokes("escape");
    }
    #[gpui::test]
    fn first_section_can_be_inserted_after_last(cx: &mut TestAppContext) {
        let (handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let (down, to) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 12)));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(
                v.sections.gesture.as_ref().unwrap().command.is_some(),
                "after-last drop must move the song"
            );
        });
        assert!(visual.debug_bounds("section-insertion-preview").is_none());
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        let event = editor.update(cx, |v, _| next(&v.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        editor.update(cx, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            let sections = v.state.as_ref().unwrap().project.arrangement_sections();
            assert_eq!(sections[0].name(), "Chorus");
            assert_eq!(sections[1].name(), "Intro");
            assert_eq!(sections[1].end(), BeatTime::from_quarters(12).unwrap());
            assert_eq!(
                v.state.as_ref().unwrap().project.meter_map(),
                &hex_project::MeterMap::common_time(),
                "dragging equal-meter sections must not create meter boundaries"
            );
        });
        assert!(
            visual
                .debug_bounds("arrangement-grid-line-192000")
                .is_some()
        );
        editor.read_with(cx, |v, _| {
            let state = v.state.as_ref().unwrap();
            let viewport = v.visible_viewport(state.timeline_frames);
            let width = v.main_track_lane_bounds.unwrap().width;
            let line = super::super::musical_time::grid_lines(
                &state.project,
                viewport,
                width,
                v.snap_grid,
            )
            .into_iter()
            .find(|line| line.frame == 192_000)
            .unwrap();
            assert!(!line.meter_marker, "the moved seam is not a meter marker");
        });
    }
    #[gpui::test]
    fn selected_sections_l_uses_outer_bounds_and_one_history_edit(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        // Use the real marker click, including the gap between these sections.
        let chorus = editor.read_with(cx, |v, _| position(v, 10));
        visual.simulate_mouse_down(chorus, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(chorus, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        for (key, start, end) in [("l", 192_000, 288_000), ("w", 0, 288_000)] {
            if start == 0 {
                let intro = editor.read_with(cx, |v, _| position(v, 2));
                let shift = Modifiers {
                    shift: true,
                    platform: true,
                    ..Modifiers::default()
                };
                visual.simulate_mouse_down(intro, MouseButton::Left, shift);
                visual.simulate_mouse_up(intro, MouseButton::Left, shift);
                visual.run_until_parked();
            }
            let before = editor.read_with(cx, |v, _| {
                assert_eq!(v.sections.selection.len(), if start == 0 { 2 } else { 1 });
                v.state.as_ref().unwrap().project.clone()
            });
            if key == "l" {
                crate::editor::section_launch::test_tap_loop_key(&mut visual);
            } else {
                visual.simulate_keystrokes(key);
            }
            editor.read_with(cx, |v, _| assert!(matches!(v.last_enqueued_edit,
                Some(Command::SetLoopRegion { start: a, end: b, enabled: true }) if (a,b)==(start,end)),
                "selected sections must dispatch a loop request"));
            let event = editor.update(cx, |v, _| next(&v.service));
            assert!(event.error.is_none(), "{:?}", event.error);
            let after = event.state.project.clone();
            let region = after.loop_region().unwrap();
            assert_eq!(
                (region.start(), region.end(), region.enabled()),
                (start, end, true)
            );
            assert_eq!(after.tracks(), before.tracks());
            assert_eq!(after.arrangement_sections(), before.arrangement_sections());
            editor.update(cx, |v, cx| {
                v.receive(event);
                cx.notify();
            });
            visual.run_until_parked();
            for (key, expected) in [("cmd-z", &before), ("cmd-shift-z", &after)] {
                visual.simulate_keystrokes(key);
                let event = editor.update(cx, |v, _| next(&v.service));
                assert!(event.error.is_none(), "{:?}", event.error);
                assert_eq!(&event.state.project, expected);
                editor.update(cx, |v, cx| {
                    v.receive(event);
                    cx.notify();
                });
                visual.run_until_parked();
            }
            editor.read_with(cx, |v, _| {
                assert!(v.sections.selection.iter().all(|id| ids.contains(id)))
            });
        }
    }

    #[gpui::test]
    fn lost_section_button_records_cancel_and_allows_loop_before_late_release(
        cx: &mut TestAppContext,
    ) {
        let directory = std::env::temp_dir().join(format!(
            "hex-section-loop-diagnostic-test-{}",
            std::process::id()
        ));
        let mut log = hex_diagnostics::SessionLog::start(directory).unwrap();
        cx.update(|cx| {
            crate::diagnostics::install_session_log(cx, Ok(log.handle()));
            crate::diagnostics::install_action_diagnostics(cx, true);
        });
        let (handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let from = editor.read_with(cx, |v, _| position(v, 2));
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        crate::editor::section_launch::test_tap_loop_key(&mut visual);
        editor.read_with(cx, |v, _| {
            assert!(v.sections.held());
            assert!(
                v.last_enqueued_edit.is_none(),
                "a real held gesture still blocks loop creation"
            );
        });
        visual.simulate_mouse_move(point(px(1.0), px(1.0)), None, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(
                !v.sections.held(),
                "button loss cancels before any late release"
            )
        });
        crate::editor::section_launch::test_tap_loop_key(&mut visual);
        editor.read_with(cx, |v, _| {
            assert!(matches!(
                v.last_enqueued_edit,
                Some(Command::SetLoopRegion { .. })
            ))
        });
        editor.update(cx, |view, cx| {
            let event = next(&view.service);
            assert!(event.error.is_none());
            view.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        let point = editor.read_with(cx, |view, _| position(view, 10));
        visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        log.shutdown().unwrap();
        let contents = std::fs::read_to_string(log.path()).unwrap();
        assert!(contents.contains("section_held"));
        assert!(contents.contains("stage=section_capture outcome=cancel"));
        assert!(contents.contains("stage=section_capture outcome=release"));
    }

    #[gpui::test]
    fn selected_sections_loop_resolves_current_tempo_and_respects_input_custody(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(cx, |v, cx| {
            v.service
                .submit(Request::Edit(Command::SetTempoPoint {
                    beat: BeatTime::from_quarters(4).unwrap(),
                    tempo: hex_project::Tempo::new(60_000_000).unwrap(),
                }))
                .unwrap();
            let event = next(&v.service);
            assert!(event.error.is_none());
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        let chorus = editor.read_with(cx, |v, _| position(v, 10));
        visual.simulate_mouse_down(chorus, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(chorus, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.update(cx, |v, _| {
            assert_eq!(v.section_loop_bounds(), Ok(Some((288_000, 480_000))));
            v.last_enqueued_edit = None;
            v.computer_keyboard.enabled = true;
        });
        crate::editor::section_launch::test_tap_loop_key(&mut visual);
        editor.update(cx, |v, _| {
            assert!(
                v.last_enqueued_edit.is_none(),
                "L retains computer MIDI priority"
            );
            v.computer_keyboard.enabled = false;
        });
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        editor.read_with(cx, |v, _| assert!(v.sections.naming()));
        visual.simulate_keystrokes("l cmd-l");
        editor.read_with(cx, |v, _| assert!(v.last_enqueued_edit.is_none()));
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        let chorus = editor.read_with(cx, |v, _| position(v, 10));
        visual.simulate_mouse_down(chorus, MouseButton::Left, Modifiers::default());
        crate::editor::section_launch::test_tap_loop_key(&mut visual);
        editor.read_with(cx, |v, _| {
            assert!(v.sections.held());
            assert!(
                v.last_enqueued_edit.is_none(),
                "held section capture cannot set a loop"
            );
        });
        visual.simulate_mouse_up(chorus, MouseButton::Left, Modifiers::default());
        editor.update(cx, |v, _| {
            // Simulate an unreconciled stale selection at the read-only boundary.
            v.service
                .submit(Request::EditArrangementSection {
                    project_generation: v.project_generation,
                    revision: v.state.as_ref().unwrap().revision,
                    command: Command::RemoveArrangementSection(ids[0]),
                })
                .unwrap();
            let event = next(&v.service);
            assert!(event.error.is_none());
            v.receive(event);
            v.sections.selection = ids.to_vec();
            v.sections.selected = Some(ids[1]);
            assert_eq!(
                v.section_loop_bounds(),
                Err("selected section no longer belongs to the arrangement")
            );
            v.sections.selection = vec![ids[1]];
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            // A retained range from another context must not win over Main sections.
            v.time_selection = Some(super::super::time_selection::TimeSelection {
                anchor_track: Some(track),
                end_track: Some(track),
                anchor_frame: 1,
                end_frame: 2,
            });
        });
        crate::editor::section_launch::test_tap_loop_key(&mut visual);
        editor.read_with(cx, |v, _| {
            assert!(matches!(
                v.last_enqueued_edit,
                Some(Command::SetLoopRegion {
                    start: 288_000,
                    end: 480_000,
                    enabled: true
                })
            ))
        });
        let event = editor.update(cx, |v, _| next(&v.service));
        assert!(event.error.is_none());
        let region = event.state.project.loop_region().unwrap();
        assert_eq!((region.start(), region.end()), (288_000, 480_000));
    }

    #[gpui::test]
    fn multiselection_hover_and_g_merge_keep_music_and_focus_guards(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let (a, b) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 10)));
        let base = painted_fill(&mut visual, 0);
        visual.simulate_event(gpui::MouseMoveEvent {
            position: a,
            pressed_button: None,
            modifiers: Modifiers::default(),
        });
        visual.run_until_parked();
        assert_ne!(
            base,
            painted_fill(&mut visual, 0),
            "hover changes section surface"
        );
        visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let selected = painted_fill(&mut visual, 0);
        assert_ne!(base, selected, "selection changes entire section surface");
        let shift = Modifiers {
            shift: true,
            platform: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(b, MouseButton::Left, shift);
        visual.simulate_mouse_up(b, MouseButton::Left, shift);
        visual.run_until_parked();
        let tracks = editor.read_with(cx, |v, _| {
            assert_eq!(v.sections.selection.len(), 2);
            assert!(v.sections.pending.is_none());
            v.state.as_ref().unwrap().project.tracks().to_vec()
        });
        visual.simulate_keystrokes("g");
        let event = editor.update(cx, |v, _| next(&v.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        editor.update(cx, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.arrangement_sections().len(), 1);
            assert_eq!(
                p.arrangement_section(ids[0]).unwrap().end(),
                BeatTime::from_quarters(12).unwrap()
            );
            assert_eq!(p.tracks(), tracks);
            assert_eq!(v.sections.selected, Some(ids[0]));
            assert_eq!(v.sections.selection, vec![ids[0]]);
        });
        // Text editing owns printable g instead of merging a section.
        visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-r");
        visual.simulate_input("g");
        editor.read_with(cx, |v, cx| {
            assert!(v.sections.naming());
            assert_eq!(v.sections.name.as_ref().unwrap().input.read(cx).text(), "g");
            assert!(v.sections.pending.is_none());
        });
    }
    #[gpui::test]
    fn content_resize_release_retains_preview_and_command_resize_only_moves_marker(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let edge = bounds(&mut visual, 0);
        let down = point(
            edge.origin.x + edge.size.width - px(1.0),
            edge.origin.y + px(12.0),
        );
        let to = editor.read_with(cx, |v, _| position(v, 6));
        let before = editor.read_with(cx, |v, _| {
            v.state.as_ref().unwrap().project.tracks().to_vec()
        });
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(matches!(
                v.sections.gesture.as_ref().unwrap().command,
                Some(Command::ResizeArrangementSectionEdge { .. })
            ))
        });
        let preview = bounds(&mut visual, 0);
        let outside = point(px(1.0), px(1.0));
        visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        let event = editor.update(cx, |v, _| next(&v.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert_eq!(bounds(&mut visual, 0), preview);
        }
        editor.update(cx, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(bounds(&mut visual, 0), preview);
        editor.read_with(cx, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(
                p.arrangement_section(ids[0]).unwrap().end(),
                BeatTime::from_quarters(6).unwrap()
            );
            assert_ne!(p.tracks(), before);
        });
        // Normal deletion cuts song time; command deletion is covered separately.
        visual.simulate_keystrokes("backspace");
        let event = editor.update(cx, |v, _| next(&v.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        editor.update(cx, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_section(ids[0])
                    .is_none()
            )
        });
    }
    #[gpui::test]
    fn section_duplicate_shortcut_copies_song_content_and_is_one_history_step(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let down = editor.read_with(cx, |v, _| position(v, 2));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
        let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_keystrokes("cmd-d");
        let event = editor.update(cx, |v, _| next(&v.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        editor.update(cx, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.arrangement_sections().len(), 3);
            assert_eq!(
                p.arrangement_sections()[1].start(),
                BeatTime::from_quarters(4).unwrap()
            );
            assert!(p.tracks()[0].midi_clips().len() > 1);
        });
        visual.simulate_keystrokes("cmd-z");
        let event = editor.update(cx, |v, _| next(&v.service));
        assert!(event.error.is_none());
        editor.update(cx, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        editor.read_with(cx, |v, _| {
            let mut actual = v.state.as_ref().unwrap().project.snapshot();
            actual.next_id = before.next_id;
            assert_eq!(actual, before);
        });
    }
    #[gpui::test]
    fn launch_action_geometry_matches_raw_exclusion_without_enlarging_tiny_sections(
        cx: &mut TestAppContext,
    ) {
        for empty in [false, true] {
            let (handle, editor, ids) = fixture(cx);
            if empty {
                editor.update(cx, |view, cx| {
                    let track = view.state.as_ref().unwrap().project.tracks()[0].id();
                    view.service
                        .submit(Request::Edit(Command::RemoveTrack(track)))
                        .unwrap();
                    let event = next(&view.service);
                    assert!(event.error.is_none());
                    view.receive(event);
                    cx.notify();
                });
            }
            let mut visual = VisualTestContext::from_window(handle, cx);
            for width in [880.0, 1440.0] {
                visual.simulate_resize(gpui::size(px(width), px(700.0)));
                for start in [0, 72_000] {
                    editor.update(cx, |view, cx| {
                        view.viewport = Some(Viewport {
                            start,
                            length: 384_000,
                        });
                        cx.notify();
                    });
                    visual.run_until_parked();
                    for (index, id) in ids.iter().enumerate() {
                        let header = visual
                            .debug_bounds(Box::leak(
                                format!("arrangement-section-header-{index}").into_boxed_str(),
                            ))
                            .unwrap();
                        let selector =
                            Box::leak(format!("section-launch-{}", id.value()).into_boxed_str());
                        let expected = super::super::section_launch::action_bounds(
                            f32::from(header.left()),
                            f32::from(header.top()),
                            f32::from(header.size.width),
                        );
                        assert_eq!(visual.debug_bounds(selector), expected);
                        if let Some(bounds) = expected {
                            let at = bounds.center();
                            editor.read_with(cx, |view, _| {
                                assert!(
                                    view.pointer_hits_section_launch(
                                        f32::from(at.x),
                                        f32::from(at.y)
                                    )
                                );
                                assert!(
                                    !view.pointer_hits_section_launch(
                                        f32::from(header.left()) + 1.0,
                                        f32::from(at.y)
                                    ),
                                    "leading resize edge remains available"
                                );
                            });
                        }
                    }
                }
            }
            let play = visual
                .debug_bounds(Box::leak(
                    format!("section-launch-{}", ids[1].value()).into_boxed_str(),
                ))
                .unwrap()
                .center();
            let before = editor.read_with(cx, |view, _| {
                view.state.as_ref().unwrap().project.snapshot()
            });
            for range_key in [false, true] {
                editor.update(cx, |view, _| {
                    view.error = None;
                    view.time_selection_key_held = range_key;
                });
                visual.simulate_mouse_down(play, MouseButton::Left, Modifiers::default());
                visual.simulate_mouse_up(play, MouseButton::Left, Modifiers::default());
                editor.read_with(cx, |view, _| {
                    assert_eq!(
                        view.error.as_deref(),
                        Some("section playback source is unavailable")
                    );
                    assert!(!view.sections.held());
                    assert!(view.time_selection_gesture.is_none());
                    assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
                });
            }
            editor.update(cx, |view, _| view.time_selection_key_held = false);
            let tiny = editor.update(cx, |view, cx| {
                let start = BeatTime::from_quarters(6).unwrap();
                view.service
                    .submit(Request::EditArrangementSection {
                        project_generation: view.project_generation,
                        revision: view.state.as_ref().unwrap().revision,
                        command: Command::AddArrangementSection {
                            name: "tiny".into(),
                            color: ElementColor::Red,
                            start,
                            end: BeatTime::from_raw(start.raw() + 1),
                        },
                    })
                    .unwrap();
                let event = next(&view.service);
                assert!(event.error.is_none());
                let Some(Created::ArrangementSection(id)) = event.created else {
                    panic!("tiny section");
                };
                view.receive(event);
                cx.notify();
                id
            });
            visual.run_until_parked();
            assert!(
                visual
                    .debug_bounds(Box::leak(
                        format!("section-launch-{}", tiny.value()).into_boxed_str()
                    ))
                    .is_none()
            );
            editor.update(cx, |view, cx| {
                view.service
                    .submit(Request::EditArrangementSection {
                        project_generation: view.project_generation,
                        revision: view.state.as_ref().unwrap().revision,
                        command: Command::RemoveArrangementSection(ids[1]),
                    })
                    .unwrap();
                let event = next(&view.service);
                assert!(event.error.is_none());
                view.receive(event);
                cx.notify();
            });
            visual.run_until_parked();
            visual.update(|window, cx| {
                let view = editor.read(cx);
                assert!(!view.sections.launch_focus.borrow().contains_key(&ids[1]));
                assert!(view.focus.is_focused(window));
            });
        }
    }

    #[gpui::test]
    fn section_autoplay_click_dispatches_stateless_loop_and_toggle_off_cancels(
        cx: &mut TestAppContext,
    ) {
        let _output_fixture_guard = crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
        use std::sync::Arc;
        let (handle, editor, ids) = fixture(cx);
        let deadline = crate::test_wait::hang_deadline();
        let (mut output, mut backend) = loop {
            match hex_audio::worker_fixture::open(48_000) {
                Ok(output) => break output,
                Err(error)
                    if error == "audio output is open or still shutting down"
                        && Instant::now() < deadline =>
                {
                    std::thread::yield_now()
                }
                Err(error) => panic!("{error}"),
            }
        };
        let region = hex_project::LoopRegion::new(0, 96_000, true).unwrap();
        editor.update(cx, |view, _| {
            view.service
                .submit(Request::Edit(Command::SetLoopRegion {
                    start: region.start(),
                    end: region.end(),
                    enabled: true,
                }))
                .unwrap();
            let event = next(&view.service);
            assert!(event.error.is_none());
            view.receive(event);
            view.pending_playback = None;
        });
        let source = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        let prepared = Arc::new(
            hex_engine::PreparedRevision::prepare(
                &source,
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        assert!(!prepared.arrangement_seek_requires_preroll());
        let audio_revision = output
            .publication
            .control()
            .unwrap()
            .publish(
                hex_engine::playback::Playback::try_from_prepared_with_session(Arc::clone(
                    &prepared,
                ))
                .unwrap(),
            )
            .unwrap();
        backend.render(&mut [[0.0; 2]; 1]).unwrap();
        let client = output.take_client().unwrap();
        editor.update(cx, |view, cx| {
            view.audio = Some(crate::EditorAudio {
                control: client,
                sample_rate: 48_000,
                label: "test audio".into(),
                spectrum_error: None,
                mode: hex_session::DocumentAudioMode::Resident,
            });
            let state = Arc::clone(view.state.as_ref().unwrap());
            view.applied_project_revision = Some(state.revision);
            view.audible_playback = Some(super::super::PlaybackCandidate {
                clip_preview_identity: None,
                worker_requires_preroll: Some(false),
                observation: None,
                audio_revision,
                project_generation: view.project_generation,
                state,
                prepared_revision: Some(prepared),
                seek_request_id: None,
                device_preview_identity: None,
                tuning_preview_identity: None,
            });
            view.session_quantization = hex_engine::LaunchQuantization::QuarterTriplet;
            view.toggle_section_autoplay();
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let clicked = editor.read_with(cx, |view, _| position(view, 1));
        visual.simulate_mouse_down(clicked, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(clicked, MouseButton::Left, Modifiers::default());
        editor.update(cx, |view, _| {
            view.tick_section_launch();
            let pending = view
                .prepared_seek
                .as_ref()
                .expect("stationary section click prepares a seek even without stateful devices");
            let section = pending.kind.section().unwrap();
            assert_eq!(section.region, region);
            assert_eq!(
                section.quantization,
                hex_engine::LaunchQuantization::QuarterTriplet
            );
            assert_eq!(view.sections.selected, Some(ids[0]));
            assert_eq!(
                view.state.as_ref().unwrap().project,
                source,
                "an already-matching loop needs no new history edit"
            );
            let cancellation = Arc::clone(&pending.worker_cancel);
            view.toggle_section_autoplay();
            assert!(cancellation.load(std::sync::atomic::Ordering::Acquire));
            assert!(view.latched_seek_intent.is_none());
            assert!(!view.section_autoplay);
        });
        // Turning autoplay off must still leave ordinary section dragging
        // available on an actually playing output.
        editor.update(cx, |view, _| {
            view.audio
                .as_mut()
                .unwrap()
                .control
                .command(hex_engine::PlaybackCommand::Play)
                .unwrap();
        });
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let end = editor.read_with(cx, |view, _| position(view, 12));
        visual.simulate_mouse_down(clicked, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(
                view.audio_status()
                    .unwrap()
                    .playing
                    .load(std::sync::atomic::Ordering::Acquire)
            );
            let gesture = view
                .sections
                .gesture
                .as_ref()
                .expect("live reorder capture");
            assert!(matches!(
                gesture.command,
                Some(Command::ReorderSectionAfter { .. })
                    | Some(Command::ReorderSectionBefore { .. })
            ));
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(view.state.as_ref().unwrap().project, source);
        });
        editor.update(cx, |view, _| {
            view.cancel_section_pointer();
        });
        visual.update(|window, cx| {
            window.cancel_pointer_capture_for(editor.entity_id(), cx);
        });
        visual.run_until_parked();
        let selector = Box::leak(format!("section-launch-{}", ids[0].value()).into_boxed_str());
        let target = visual
            .debug_bounds(selector)
            .expect("explicit section launch")
            .center();
        let selected = editor.read_with(cx, |view, _| view.sections.selected);
        visual.update(|window, cx| {
            editor.read(cx).sections.launch_focus.borrow()[&ids[0]].focus(window)
        });
        visual.simulate_keystrokes("enter");
        editor.read_with(cx, |view, _| {
            let pending = view
                .pending_loop_region()
                .expect("enter disables the old loop");
            assert_eq!(
                pending,
                hex_project::LoopRegion::new(0, 96_000, false).unwrap()
            );
            assert!(!view.section_autoplay);
            assert!(!view.sections.held());
            assert_eq!(view.sections.selected, selected);
            assert_eq!(view.state.as_ref().unwrap().project, source);
        });
        editor.update(cx, |view, _| {
            view.cancel_section_launch_wait();
            view.invalidate_prepared_seek();
            view.save_dialog_open = true;
        });
        visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| {
            assert!(!view.section_launch_waiting());
            assert!(view.latched_seek_intent.is_none());
            assert!(
                view.prepared_seek
                    .as_ref()
                    .is_none_or(|pending| pending.cancelled)
            );
        });
        editor.update(cx, |view, _| {
            view.save_dialog_open = false;
            view.audio = None;
        });
    }

    #[gpui::test]
    fn stationary_non_grid_content_edge_click_does_not_change_music(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(cx, |v, cx| {
            v.service
                .submit(Request::EditArrangementSection {
                    project_generation: v.project_generation,
                    revision: v.state.as_ref().unwrap().revision,
                    command: Command::SetArrangementSectionRange {
                        section: ids[0],
                        start: BeatTime::ZERO,
                        end: BeatTime::from_ratio(19, 5).unwrap(),
                    },
                })
                .unwrap();
            let event = next(&v.service);
            assert!(event.error.is_none());
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        let edge = bounds(&mut visual, 0);
        let down = point(
            edge.origin.x + edge.size.width - px(1.0),
            edge.origin.y + px(12.0),
        );
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |v, _| {
            assert!(v.sections.pending.is_none());
            assert_eq!(v.worker_requests_pending, 0);
        });
    }
    #[gpui::test]
    fn stationary_click_near_snapped_end_does_not_move_song(cx: &mut TestAppContext) {
        let (handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let a = bounds(&mut visual, 0);
        let down = point(a.origin.x + a.size.width - px(7.0), a.origin.y + px(12.0));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |v, _| {
            assert!(v.sections.pending.is_none());
            assert_eq!(v.worker_requests_pending, 0);
        });
    }
    #[gpui::test]
    fn option_command_move_changes_canonical_marker_without_moving_music(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let tracks = editor.read_with(cx, |v, _| {
            v.state.as_ref().unwrap().project.tracks().to_vec()
        });
        let (down, to) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 3)));
        let modifiers = Modifiers {
            platform: true,
            alt: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
        visual.simulate_mouse_move(to, MouseButton::Left, modifiers);
        visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
        let event = editor.update(cx, |v, _| next(&v.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        editor.update(cx, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.tracks(), tracks);
            assert!(p.arrangement_section(ids[0]).unwrap().start() > BeatTime::ZERO);
            assert_eq!(
                p.arrangement_section(ids[0]).unwrap().end().raw()
                    - p.arrangement_section(ids[0]).unwrap().start().raw(),
                BeatTime::from_quarters(4).unwrap().raw()
            );
        });
    }
    #[gpui::test]
    fn reorder_pending_keeps_completed_frames_until_its_exact_terminal(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let (down, to) = editor.read_with(cx, |v, _| (position(v, 10), position(v, 0)));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("section-insertion-preview").is_none());
        let preview = bounds(&mut visual, 1);
        // Controlled FIFO seam: reserve two ordinary completion ordinals,
        // then deliver real earlier-work and refused section Service events.
        // The separate success test exercises real release/enqueue stamping.
        editor.update(cx, |v, _| {
            let g = v.sections.gesture.take().unwrap();
            let command = g.command.unwrap();
            assert!(matches!(command,
                Command::ReorderSectionBefore { section, before } if section == ids[1] && before == ids[0]
            ));
            let mut receipt = Receipt::new(v.project_generation);
            assert!(receipt.submitted(2));
            v.sections.pending = Some(Pending {
                selection_intent_epoch: v.selection_intent_epoch,
                generation: v.project_generation,
                revision: g.revision,
                command,
                display: g.display,
                layout: None,
                copied: None,
                receipt,
            });
            v.worker_requests_pending = 2;
        });
        visual.simulate_keystrokes("escape");
        editor.update(cx, |v, _| {
            v.toggle_play();
            assert!(format!("{:?}", v.error).contains("applying edit"));
            v.request_seek(12);
            assert!(format!("{:?}", v.error).contains("applying edit"));
            assert!(v.pending_audio_commands.is_empty());
        });
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert_eq!(bounds(&mut visual, 1), preview);
            assert!(visual.debug_bounds("section-insertion-preview").is_none());
        }
        let earlier = editor.update(cx, |v, _| {
            v.service.submit(Request::Undo).unwrap();
            next(&v.service)
        });
        let terminal = editor.update(cx, |v, _| {
            let p = v.sections.pending.as_ref().unwrap();
            v.service
                .submit(Request::EditArrangementSection {
                    project_generation: p.generation,
                    revision: p.revision,
                    command: p.command.clone(),
                })
                .unwrap();
            next(&v.service)
        });
        // Earlier completion keeps the frozen paint, even if it changes revision.
        editor.update(cx, |v, cx| {
            v.receive(earlier);
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(bounds(&mut visual, 1), preview);
        // Undo made the stamp stale: refusal and canonical install retire together.
        assert!(terminal.error.is_some());
        editor.update(cx, |v, cx| {
            v.receive(terminal);
            cx.notify();
        });
        visual.run_until_parked();
        assert!(visual.debug_bounds("section-insertion-preview").is_none());
        editor.read_with(cx, |v, _| assert!(v.sections.pending.is_none()));
    }
    fn midi_clip_bounds(
        visual: &mut VisualTestContext,
        project: &hex_project::Project,
    ) -> Vec<(u64, gpui::Bounds<gpui::Pixels>)> {
        project.tracks()[0]
            .midi_clips()
            .iter()
            .map(|clip| {
                let id = clip.id().value();
                let selector = Box::leak(format!("midi-clip-visual-{id}").into_boxed_str());
                (id, visual.debug_bounds(selector).expect("painted clip"))
            })
            .collect()
    }
    #[gpui::test]
    fn pending_reorder_draws_final_clips_until_exact_terminal_and_refusal_restores(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let canonical = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.clone());
        let original = midi_clip_bounds(&mut visual, &canonical);
        let (down, to) = editor.read_with(cx, |v, _| (position(v, 10), position(v, 2)));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        let terminal = editor.update(cx, |v, _| {
            assert!(v.sections.pending.as_ref().unwrap().layout.is_some());
            next(&v.service)
        });
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        let committed = std::sync::Arc::clone(&terminal.state.project);
        assert_ne!(*committed, *canonical, "reorder moves clip content");
        // Delayed ACK: every completed repaint already shows the final clips.
        let mut released = Vec::new();
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            released = midi_clip_bounds(&mut visual, &committed);
            assert_ne!(released, original);
        }
        editor.update(cx, |v, cx| {
            v.receive(terminal);
            assert!(v.sections.pending.is_none());
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(midi_clip_bounds(&mut visual, &committed), released);

        // A stale stamp previews the move, then its refusal restores canonical clips.
        let (down, to) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 10)));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let renamed = editor.update(cx, |v, _| {
            v.service
                .submit(Request::Edit(Command::RenameTrack {
                    track: committed.tracks()[0].id(),
                    name: "renamed".into(),
                }))
                .unwrap();
            next(&v.service)
        });
        assert!(renamed.error.is_none(), "{:?}", renamed.error);
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        let refused = editor.update(cx, |v, _| next(&v.service));
        assert!(refused.error.is_some(), "stale section stamp");
        visual.update(|window, _| window.refresh());
        visual.run_until_parked();
        assert_ne!(midi_clip_bounds(&mut visual, &committed), released);
        editor.update(cx, |v, cx| {
            // The refusal carries the newer canonical state (the rename).
            v.receive(refused);
            assert!(v.sections.pending.is_none());
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(midi_clip_bounds(&mut visual, &committed), released);
    }
    #[gpui::test]
    fn section_name_palette_delete_and_text_focus_are_guarded(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let down = editor.read_with(cx, |v, _| position(v, 2));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(v.sections.naming());
            assert!(v.clip_name_edit.is_none());
        });
        visual.simulate_keystrokes("backspace");
        editor.read_with(cx, |v, cx| {
            assert!(
                v.sections
                    .name
                    .as_ref()
                    .unwrap()
                    .input
                    .read(cx)
                    .text()
                    .is_empty()
            );
            assert!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_section(ids[0])
                    .is_some()
            );
        });
        visual.update(|window, cx| {
            editor.update(cx, |v, cx| {
                assert!(!v.finalize_section_name_for_open(window, cx));
                assert!(v.sections.naming());
                v.new_project(hex_session::DocumentAudioMode::Resident, window, cx);
                assert!(!v.project_open.busy());
                assert!(v.sections.naming());
                assert!(!v.opening_project);
            })
        });
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        visual.simulate_mouse_down(down, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            let menu = v.context_menu.as_ref().unwrap();
            let choices: Vec<_> = menu
                .items
                .iter()
                .filter_map(|item| {
                    if let super::super::context_menu::EditorContextAction::Section(MenuAction {
                        kind: MenuKind::Color(color),
                        ..
                    }) = item.action
                    {
                        Some(color)
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(choices, ElementColor::TRACK_CHOICES);
        });
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-r");
        visual.simulate_input("Bridge X");
        visual.run_until_parked();
        editor.read_with(cx, |v, cx| {
            let input = v.sections.name.as_ref().unwrap().input.read(cx);
            assert_eq!(input.text(), "Bridge X");
            assert_eq!(input.painted_text(), Some("bridge x"));
        });
        visual.simulate_keystrokes("enter");
        let terminal = editor.update(cx, |v, _| next(&v.service));
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        editor.update(cx, |v, cx| {
            v.receive(terminal);
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_section(ids[0])
                    .unwrap()
                    .name(),
                "Bridge X"
            );
            cx.notify();
        });
        visual.run_until_parked();
        let original_fill = painted_fill(&mut visual, 0);
        visual.simulate_mouse_down(down, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        let green = editor.read_with(cx, |v, _| {
            v.context_menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .position(|item| item.label == "green")
                .unwrap()
        });
        for _ in 0..green {
            visual.simulate_keystrokes("down");
        }
        visual.simulate_keystrokes("enter");
        let terminal = editor.update(cx, |v, _| next(&v.service));
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        editor.update(cx, |v, cx| {
            v.receive(terminal);
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_section(ids[0])
                    .unwrap()
                    .color(),
                ElementColor::Green
            );
            cx.notify();
        });
        visual.run_until_parked();
        assert_ne!(painted_fill(&mut visual, 0), original_fill);
        let tracks = editor.read_with(cx, |v, _| {
            v.state.as_ref().unwrap().project.tracks().to_vec()
        });
        visual.simulate_keystrokes("alt-cmd-backspace");
        let terminal = editor.update(cx, |v, _| next(&v.service));
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        editor.update(cx, |v, cx| {
            v.receive(terminal);
            assert!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_section(ids[0])
                    .is_none()
            );
            assert_eq!(v.state.as_ref().unwrap().project.tracks(), tracks);
            cx.notify();
        });
        // A stale menu action cannot recolor any new canonical source.
        editor.update(cx, |v, _| {
            let state = v.state.as_ref().unwrap();
            let action = MenuAction {
                generation: v.project_generation,
                revision: state.revision,
                section: ids[1],
                kind: MenuKind::Color(ElementColor::Green),
            };
            v.project_generation += 1;
            assert!(v.section_menu_stale(&action));
            assert!(v.sections.pending.is_none());
        });
    }
    #[gpui::test]
    fn painted_section_release_uses_stamped_queue_and_keeps_success_preview(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let (start, end) = editor.read_with(cx, |v, _| (position(v, 5), position(v, 7)));
        visual.simulate_event(MouseDownEvent {
            position: start,
            button: MouseButton::Left,
            click_count: 2,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let preview = bounds(&mut visual, 2);
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        let terminal = editor.update(cx, |v, _| {
            assert!(!v.sections.held());
            let p = v
                .sections
                .pending
                .as_ref()
                .expect("released pending display");
            assert!(
                !p.receipt.is_queued(),
                "immediate enqueue stamps the exact receipt"
            );
            next(&v.service)
        });
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert_eq!(bounds(&mut visual, 2), preview);
        }
        editor.update(cx, |v, cx| {
            v.receive(terminal);
            assert!(v.sections.pending.is_none());
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_sections()
                    .len(),
                3
            );
            cx.notify();
        });
        visual.run_until_parked();
        // The ID becomes project owned and rows re-sort, preserving the final
        // geometry through the canonical installation in the same receive.
        assert_eq!(bounds(&mut visual, 1), preview);
        let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.clone());
        let (source, target) = editor.read_with(cx, |v, _| (position(v, 10), position(v, 0)));
        visual.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("section-insertion-preview").is_none());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        let terminal = editor.update(cx, |v, _| next(&v.service));
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        editor.update(cx, |v, cx| {
            v.receive(terminal);
            let sections = v.state.as_ref().unwrap().project.arrangement_sections();
            assert_eq!(sections[0].name(), "Chorus");
            assert_eq!(sections[1].name(), "Intro");
            cx.notify();
        });
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-z");
        let terminal = editor.update(cx, |v, _| next(&v.service));
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        editor.update(cx, |v, cx| {
            v.receive(terminal);
            let restored = v.state.as_ref().unwrap().project.snapshot();
            let mut expected = before.snapshot();
            assert!(
                restored.next_id >= expected.next_id,
                "undo never reuses project identities"
            );
            expected.next_id = restored.next_id;
            assert_eq!(
                restored, expected,
                "one undo restores every musical and structural field"
            );
            cx.notify();
        });
    }
    #[gpui::test]
    fn empty_track_list_can_paint_sections_and_queue_refusal_retires_display(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, _) = fixture(cx);
        editor.update(cx, |v, cx| {
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            v.service
                .submit(Request::Edit(Command::RemoveTrack(track)))
                .unwrap();
            let event = next(&v.service);
            assert!(event.error.is_none());
            v.state = Some(event.state);
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let (start, end) = editor.read_with(cx, |v, _| (position(v, 5), position(v, 7)));
        visual.simulate_event(MouseDownEvent {
            position: start,
            button: MouseButton::Left,
            click_count: 2,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(bounds(&mut visual, 2).size.width > px(1.0));
        // Held gesture then queue saturation between update and submission:
        // the immediate refusal must retire released paint synchronously.
        editor.update(cx, |v, cx| {
            let g = v.sections.gesture.take().unwrap();
            let command = g.command.unwrap();
            for _ in 0..super::super::UI_REQUEST_CAPACITY {
                v.pending_requests.push_back(Request::Undo);
            }
            v.submit_section(command, g.display, g.generation, g.revision);
            assert!(v.sections.pending.is_none());
            v.pending_requests.clear();
            cx.notify();
        });
        visual.run_until_parked();
        assert!(visual.debug_bounds("arrangement-section-2").is_none());
    }
    #[gpui::test]
    fn ordinary_clip_and_text_focus_release_section_keyboard_ownership(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let section = editor.read_with(cx, |v, _| position(v, 2));
        visual.simulate_mouse_down(section, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(section, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |v, _| assert_eq!(v.sections.selected, Some(ids[0])));
        let clip = editor.read_with(cx, |v, _| {
            v.state.as_ref().unwrap().project.tracks()[0].midi_clips()[0].id()
        });
        let header = visual
            .debug_bounds(Box::leak(
                format!("clip-name-{}", clip.value()).into_boxed_str(),
            ))
            .expect("ordinary clip name");
        visual.simulate_mouse_down(header.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(header.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(v.sections.selected.is_none());
            assert_eq!(v.selected_clip, Some(clip));
        });
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(v.clip_name_edit.is_some());
            assert!(!v.sections.naming());
        });
        visual.simulate_keystrokes("escape backspace");
        let terminal = editor.update(cx, |v, _| next(&v.service));
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        editor.update(cx, |v, cx| {
            v.receive(terminal);
            let project = &v.state.as_ref().unwrap().project;
            assert!(project.midi_clip(clip).is_none());
            assert_eq!(project.arrangement_sections().len(), 2);
            cx.notify();
        });
        visual.run_until_parked();
        let section = editor.read_with(cx, |v, _| position(v, 2));
        visual.simulate_mouse_down(section, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(section, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |v, _| assert_eq!(v.sections.selected, Some(ids[0])));
        visual.update(|window, cx| {
            editor.read_with(cx, |v, cx| v.browser.search.read(cx).focus(window));
            window.refresh();
        });
        visual.run_until_parked();
        editor.read_with(cx, |v, _| assert!(v.sections.selected.is_none()));
        visual.simulate_keystrokes("cmd-r backspace");
        editor.read_with(cx, |v, _| {
            assert!(!v.sections.naming());
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_sections()
                    .len(),
                2
            );
        });
        let return_track = editor.update(cx, |v, cx| {
            v.service
                .submit(Request::Edit(Command::AddReturnTrack {
                    name: "return".into(),
                }))
                .unwrap();
            let event = next(&v.service);
            assert!(event.error.is_none());
            let Some(Created::ReturnTrack(id)) = event.created else {
                panic!("return result")
            };
            v.receive(event);
            cx.notify();
            id
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            editor.update(cx, |v, cx| {
                v.focus.focus(window);
                v.select_section_metadata(Some(ids[0]));
                assert!(v.section_has_focus(window));
                // Keyboard/programmatic device focus preserves the ordinary track
                // selection and therefore must explicitly supersede Main ownership.
                assert!(v.focus_return_details(return_track, window, cx));
                assert!(!v.section_has_focus(window));
                assert!(!v.delete_section(window));
                assert!(!v.section_name_begin(window, cx));
                assert!(v.sections.pending.is_none());
                cx.notify();
            })
        });
        visual.run_until_parked();
        editor.read_with(cx, |v, _| assert!(v.sections.selected.is_none()));
        visual.update(|window, cx| {
            editor.update(cx, |v, cx| {
                v.select_section_metadata(Some(ids[0]));
                v.device_panel.main_selected = None;
                assert!(!v.section_has_focus(window));
                assert!(!v.delete_section(window));
                assert!(!v.section_name_begin(window, cx));
                assert_eq!(
                    v.state
                        .as_ref()
                        .unwrap()
                        .project
                        .arrangement_sections()
                        .len(),
                    2
                );
                cx.notify();
            })
        });
    }
    fn touching_fixture(
        cx: &mut TestAppContext,
    ) -> (gpui::AnyWindowHandle, Entity<EditorView>, [SectionId; 2]) {
        let (handle, editor, ids) = fixture(cx);
        editor.update(cx, |v, cx| {
            v.service
                .submit(Request::EditArrangementSection {
                    project_generation: v.project_generation,
                    revision: v.state.as_ref().unwrap().revision,
                    command: Command::SetArrangementSectionRange {
                        section: ids[1],
                        start: BeatTime::from_quarters(4).unwrap(),
                        end: BeatTime::from_quarters(8).unwrap(),
                    },
                })
                .unwrap();
            let event = next(&v.service);
            assert!(event.error.is_none(), "{:?}", event.error);
            v.receive(event);
            cx.notify();
        });
        (handle, editor, ids)
    }

    #[gpui::test]
    fn section_cursor_zones_distinguish_inner_edges_and_shared_seam(cx: &mut TestAppContext) {
        let (handle, _editor, _) = touching_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let left = bounds(&mut visual, 0);
        let seam = left.right();
        for (offset, cursor) in [
            (-3.0, gpui::CursorStyle::ResizeRightBracket),
            (0.0, gpui::CursorStyle::ResizeLeftRight),
            (3.0, gpui::CursorStyle::ResizeLeftBracket),
        ] {
            visual.simulate_mouse_move(
                point(seam + px(offset), left.center().y),
                None,
                Modifiers::default(),
            );
            visual.run_until_parked();
            assert_eq!(
                visual.rendered_cursor_style(),
                Some(cursor),
                "zone {offset}"
            );
        }
    }
    #[gpui::test]
    fn command_snap_changes_live_without_entering_marker_mode(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let edge = bounds(&mut visual, 0);
        let down = point(edge.right() - px(1.0), edge.center().y);
        let to = editor.read_with(cx, |v, _| {
            let a = position(v, 6);
            let b = position(v, 7);
            point(a.x + (b.x - a.x) * 0.37, a.y)
        });
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        for free in [false, true, false, true] {
            visual.simulate_modifiers_change(Modifiers {
                platform: free,
                ..Modifiers::default()
            });
            editor.read_with(cx, |v, _| {
                let Some(Command::ResizeArrangementSectionEdge {
                    section, boundary, ..
                }) = &v.sections.gesture.as_ref().unwrap().command
                else {
                    panic!("command must invert snapping without selecting marker editing");
                };
                assert_eq!(*section, ids[0]);
                if free {
                    assert!(*boundary > BeatTime::from_quarters(6).unwrap());
                    assert!(*boundary < BeatTime::from_quarters(7).unwrap());
                } else {
                    assert_eq!(*boundary, BeatTime::from_quarters(6).unwrap());
                }
            });
        }
        visual.simulate_keystrokes("escape");
        editor.read_with(cx, |v, _| assert!(v.sections.gesture.is_none()));
    }

    #[gpui::test]
    fn control_alias_inverts_snap_and_option_command_selects_marker_resize(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        for marker_only in [false, true] {
            for free in [false, true] {
                let edge = bounds(&mut visual, 0);
                let down = point(edge.right() - px(1.0), edge.center().y);
                let to = editor.read_with(cx, |v, _| {
                    let a = position(v, 6);
                    let b = position(v, 7);
                    point(a.x + (b.x - a.x) * 0.37, a.y)
                });
                let modifiers = Modifiers {
                    platform: marker_only,
                    alt: marker_only,
                    control: free,
                    ..Modifiers::default()
                };
                visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
                visual.simulate_mouse_move(to, MouseButton::Left, modifiers);
                editor.read_with(cx, |v, _| {
                    let command = v
                        .sections
                        .gesture
                        .as_ref()
                        .unwrap()
                        .command
                        .as_ref()
                        .unwrap();
                    let value = match command {
                        Command::ResizeArrangementSectionMarker {
                            section,
                            boundary: end,
                            ..
                        } if marker_only && *section == ids[0] => *end,
                        Command::ResizeArrangementSectionEdge {
                            section, boundary, ..
                        } if !marker_only && *section == ids[0] => *boundary,
                        other => panic!("wrong modifier mode: {other:?}"),
                    };
                    if free {
                        assert!(
                            value > BeatTime::from_quarters(6).unwrap()
                                && value < BeatTime::from_quarters(7).unwrap()
                        );
                    } else {
                        assert_eq!(value, BeatTime::from_quarters(6).unwrap());
                    }
                });
                visual.simulate_keystrokes("escape");
                visual.run_until_parked();
            }
        }
    }

    #[gpui::test]
    fn shared_seam_drags_publish_both_edges_atomically_and_option_command_preserves_music(
        cx: &mut TestAppContext,
    ) {
        for marker_only in [false, true] {
            let (handle, editor, ids) = touching_fixture(cx);
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
            let left = bounds(&mut visual, 0);
            let down = point(left.right(), left.center().y);
            let to = editor.read_with(cx, |v, _| position(v, 5));
            let modifiers = Modifiers {
                platform: marker_only,
                alt: marker_only,
                ..Modifiers::default()
            };
            visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
            visual.simulate_mouse_move(to, MouseButton::Left, modifiers);
            visual.run_until_parked();
            editor.read_with(cx, |v, _| {
                let command = v
                    .sections
                    .gesture
                    .as_ref()
                    .unwrap()
                    .command
                    .as_ref()
                    .unwrap();
                match command {
                    Command::ResizeArrangementSectionMarker {
                        section,
                        edge: hex_project::TrimEdge::Right,
                        boundary,
                    } if marker_only
                        && *section == ids[0]
                        && *boundary == BeatTime::from_quarters(5).unwrap() => {}
                    Command::ResizeArrangementSectionBoundary {
                        left,
                        right,
                        boundary,
                        marker_only: false,
                    } if !marker_only
                        && *left == ids[0]
                        && *right == ids[1]
                        && *boundary == BeatTime::from_quarters(5).unwrap() => {}
                    other => panic!("wrong seam command {other:?}"),
                }
            });
            let preview = [bounds(&mut visual, 0), bounds(&mut visual, 1)];
            visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
            let event = editor.update(cx, |v, _| next(&v.service));
            assert!(event.error.is_none(), "{:?}", event.error);
            for _ in 0..3 {
                visual.update(|window, _| window.refresh());
                visual.run_until_parked();
                assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
                assert!(visual.debug_bounds("section-insertion-preview").is_none());
            }
            editor.update(cx, |v, cx| {
                v.receive(event);
                cx.notify();
            });
            visual.run_until_parked();
            assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
            editor.read_with(cx, |v, _| {
                let p = &v.state.as_ref().unwrap().project;
                assert_eq!(
                    p.arrangement_section(ids[0]).unwrap().start(),
                    BeatTime::ZERO
                );
                assert_eq!(
                    p.arrangement_section(ids[0]).unwrap().end(),
                    BeatTime::from_quarters(5).unwrap()
                );
                assert_eq!(
                    p.arrangement_section(ids[1]).unwrap().start(),
                    BeatTime::from_quarters(5).unwrap()
                );
                assert_eq!(
                    p.arrangement_section(ids[1]).unwrap().end(),
                    BeatTime::from_quarters(8).unwrap()
                );
                assert_eq!(p.snapshot().tracks == before.tracks, marker_only);
            });
            visual.simulate_keystrokes("cmd-z");
            let event = editor.update(cx, |v, _| next(&v.service));
            assert!(event.error.is_none());
            editor.update(cx, |v, cx| {
                v.receive(event);
                cx.notify();
            });
            let mut undone =
                editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
            undone.next_id = before.next_id;
            assert_eq!(undone, before);
        }
    }
    #[gpui::test]
    fn option_command_after_shared_seam_capture_can_resize_past_the_neighbor(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, ids) = touching_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let left = bounds(&mut visual, 0);
        let down = point(left.right(), left.center().y);
        let to = editor.read_with(cx, |v, _| position(v, 10));
        let marker_modifier = Modifiers {
            platform: true,
            alt: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_modifiers_change(marker_modifier);
        visual.simulate_mouse_move(to, MouseButton::Left, marker_modifier);
        visual.simulate_mouse_up(to, MouseButton::Left, marker_modifier);
        editor.update(cx, |v, cx| {
            let event = next(&v.service);
            assert!(event.error.is_none(), "{:?}", event.error);
            v.receive(event);
            let after = v.state.as_ref().unwrap().project.snapshot();
            assert_eq!(after.arrangement_sections.len(), 1);
            assert_eq!(after.arrangement_sections[0].id, ids[0].value());
            assert_eq!(
                after.arrangement_sections[0].end,
                BeatTime::from_quarters(10).unwrap()
            );
            let mut music = after;
            music.arrangement_sections = before.arrangement_sections.clone();
            assert_eq!(music, before);
            cx.notify();
        });
    }

    #[gpui::test]
    fn inner_zones_resize_only_the_touched_edge_and_leave_the_neighbor_in_place(
        cx: &mut TestAppContext,
    ) {
        for (offset, target, index) in [(-3.0, 3, 0usize), (3.0, 5, 1usize)] {
            for marker_only in [false, true] {
                let (handle, editor, ids) = touching_fixture(cx);
                let mut visual = VisualTestContext::from_window(handle, cx);
                visual.run_until_parked();
                let left = bounds(&mut visual, 0);
                let neighbor = editor.read_with(cx, |v, _| {
                    v.state
                        .as_ref()
                        .unwrap()
                        .project
                        .arrangement_section(ids[1 - index])
                        .unwrap()
                        .clone()
                });
                let down = point(left.right() + px(offset), left.center().y);
                let to = editor.read_with(cx, |v, _| position(v, target));
                let modifiers = Modifiers {
                    platform: marker_only,
                    alt: marker_only,
                    ..Modifiers::default()
                };
                visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
                visual.simulate_mouse_move(to, MouseButton::Left, modifiers);
                visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
                let event = editor.update(cx, |v, _| next(&v.service));
                assert!(event.error.is_none(), "{:?}", event.error);
                editor.update(cx, |v, cx| {
                    v.receive(event);
                    cx.notify();
                });
                editor.read_with(cx, |v, _| {
                    let p = &v.state.as_ref().unwrap().project;
                    assert_eq!(p.arrangement_section(ids[1 - index]), Some(&neighbor));
                    let section = p.arrangement_section(ids[index]).unwrap();
                    assert_eq!(
                        if index == 0 {
                            section.end()
                        } else {
                            section.start()
                        },
                        BeatTime::from_quarters(target).unwrap()
                    );
                });
            }
        }
    }
    #[gpui::test]
    fn lane_cursor_splits_only_markers_and_holds_preview_until_exact_ack(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let at = editor.read_with(cx, |v, _| position(v, 2));
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("section-edit-cursor").is_some());
        assert!(visual.debug_bounds("section-insertion-preview").is_none());
        editor.read_with(cx, |v, _| {
            assert_eq!(
                v.sections.cursor.unwrap().beat,
                BeatTime::from_quarters(2).unwrap()
            );
            assert!(v.sections.pending.is_none());
        });
        visual.simulate_keystrokes("e");
        let preview = [bounds(&mut visual, 0), bounds(&mut visual, 1)];
        assert_eq!(preview[0].right(), preview[1].left());
        let event = editor.update(cx, |v, _| next(&v.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
            editor.read_with(cx, |v, _| assert!(v.sections.pending.is_some()));
        }
        editor.update(cx, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
        editor.read_with(cx, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.arrangement_sections().len(), 3);
            assert_eq!(p.arrangement_sections()[0].id(), ids[0]);
            assert_ne!(p.arrangement_sections()[1].id(), ids[0]);
            let mut after = p.snapshot();
            after.next_id = before.next_id;
            after.arrangement_sections = before.arrangement_sections.clone();
            assert_eq!(after, before);
            assert!(v.sections.pending.is_none());
        });
        visual.simulate_keystrokes("cmd-z");
        let event = editor.update(cx, |v, _| next(&v.service));
        assert!(event.error.is_none());
        editor.update(cx, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        let mut undone = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
        undone.next_id = before.next_id;
        assert_eq!(undone, before);
    }

    #[gpui::test]
    fn lane_cursor_gap_endpoint_stale_and_text_context_refuse_without_clip_fallthrough(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
        for beat in [6, 4, 0] {
            let at = editor.read_with(cx, |v, _| position(v, beat));
            visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert!(visual.debug_bounds("section-edit-cursor").is_some());
            visual.simulate_keystrokes("e");
            editor.read_with(cx, |v, _| {
                assert!(v.sections.pending.is_none());
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
                assert!(
                    v.error
                        .as_ref()
                        .is_some_and(|e| e.contains("strictly inside"))
                );
            });
        }
        let at = editor.read_with(cx, |v, _| position(v, 2));
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        editor.update(cx, |v, _| v.sections.cursor.as_mut().unwrap().revision += 1);
        visual.simulate_keystrokes("e");
        editor.read_with(cx, |v, _| {
            assert!(v.sections.pending.is_none());
            assert!(
                v.error
                    .as_ref()
                    .is_some_and(|e| e.contains("click the lane again"))
            );
        });
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        editor.read_with(cx, |v, _| assert!(v.sections.naming()));
        visual.simulate_keystrokes("cmd-a e");
        editor.read_with(cx, |v, cx| {
            assert!(v.sections.pending.is_none());
            assert_eq!(v.sections.name.as_ref().unwrap().input.read(cx).text(), "e");
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        // A regular-track point releases the lane and preserves clip splitting.
        let at = editor.read_with(cx, |v, _| {
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            let lane = v.drag_lanes[&track];
            point(px(lane.left + lane.width * 0.1), px(lane.top + 18.0))
        });
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |v, _| assert!(v.sections.cursor.is_none()));
        visual.simulate_keystrokes("e");
        let event = editor.update(cx, |v, _| next(&v.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        assert_eq!(event.state.project.arrangement_sections().len(), 2);
        assert_eq!(event.state.project.tracks()[0].midi_clips().len(), 2);
    }

    #[gpui::test]
    fn lane_cursor_snap_inversion_and_empty_project_clicks(cx: &mut TestAppContext) {
        let (handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let mut beats = Vec::new();
        for grid in [SnapGrid::Quarter, SnapGrid::Off] {
            editor.update(cx, |v, _| v.snap_grid = grid);
            for free in [false, true] {
                let at = editor.read_with(cx, |v, _| {
                    let a = position(v, 2);
                    let b = position(v, 3);
                    point(a.x + (b.x - a.x) * 0.37, a.y)
                });
                let modifiers = Modifiers {
                    platform: free,
                    ..Modifiers::default()
                };
                visual.simulate_mouse_down(at, MouseButton::Left, modifiers);
                visual.simulate_mouse_up(at, MouseButton::Left, modifiers);
                editor.read_with(cx, |v, _| {
                    let beat = v.sections.cursor.unwrap().beat;
                    beats.push(beat);
                    assert!(v.sections.pending.is_none());
                });
            }
        }
        assert_eq!(beats[0], BeatTime::from_quarters(2).unwrap());
        assert!(beats[1] > beats[0] && beats[1] < BeatTime::from_quarters(3).unwrap());
        assert_eq!(
            beats[1], beats[2],
            "off and Command-inverted fixed grid are free"
        );
        assert_ne!(
            beats[3], beats[2],
            "Command with snap off enables adaptive snapping"
        );
        editor.update(cx, |v, cx| {
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            v.service
                .submit(Request::Edit(Command::RemoveTrack(track)))
                .unwrap();
            let event = next(&v.service);
            assert!(event.error.is_none());
            v.state = Some(event.state);
            cx.notify();
        });
        visual.run_until_parked();
        let at = editor.read_with(cx, |v, _| position(v, 6));
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("section-edit-cursor").is_some());
    }
    #[gpui::test]
    fn section_lower_body_paints_all_tracks_without_section_edit_even_when_focused(
        cx: &mut TestAppContext,
    ) {
        for focused in [false, true] {
            let (handle, editor, ids) = fixture(cx);
            editor.update(cx, |view, cx| {
                assert!(view.enqueue(Request::Edit(Command::AddTrack {
                    name: "hidden empty track".into()
                })));
                let event = next(&view.service);
                assert!(event.error.is_none());
                view.receive(event);
                view.section_autoplay = false;
                view.last_enqueued_edit = None;
                cx.notify();
            });
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            let (header, start, end) = editor.read_with(cx, |view, _| {
                let [_, top, _, height] = view.section_layout().unwrap();
                let a = position(view, 1);
                let b = position(view, 3);
                (
                    point(a.x, px(top + 6.0)),
                    point(a.x, px(top + height * 0.75)),
                    point(b.x, px(top + height * 0.75)),
                )
            });
            visual.simulate_mouse_down(header, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(header, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(cx, |view, _| {
                assert_eq!(view.sections.selected, Some(ids[0]))
            });
            if focused {
                visual.simulate_keystrokes("q");
                visual.run_until_parked();
            }
            let before =
                editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
            let hidden = before.tracks()[1].id();
            if focused {
                assert!(
                    visual
                        .debug_bounds(Box::leak(
                            format!("arrangement-lane-{}", hidden.value()).into_boxed_str()
                        ))
                        .is_none()
                );
            }
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            editor.read_with(cx, |view, _| {
                assert!(
                    view.sections.gesture.is_none(),
                    "lower section body must delegate to the time-selection owner"
                );
                assert!(view.time_selection_gesture.is_some());
                assert!(
                    view.sections.cursor.is_none(),
                    "body range must not retain a second section edit cursor"
                );
            });
            visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert!(visual.debug_bounds("main-project-time-selection").is_some());
            visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(cx, |view, _| {
                let range = view
                    .normalized_time_selection()
                    .expect("completed main range");
                let map = before.tempo_map();
                assert_eq!(
                    (range.start(), range.end()),
                    (
                        map.beat_to_frame(BeatTime::ONE).unwrap(),
                        map.beat_to_frame(BeatTime::from_quarters(3).unwrap())
                            .unwrap()
                    )
                );
                assert_eq!(
                    range.tracks(),
                    before
                        .tracks()
                        .iter()
                        .map(|track| track.id())
                        .collect::<Vec<_>>()
                );
                assert!(view.time_selection_project_scope);
                assert!(view.sections.pending.is_none());
                assert!(view.last_enqueued_edit.is_none());
                assert_eq!(view.state.as_ref().unwrap().project, before);
                assert_eq!(view.arrangement_focus.projection.enabled, focused);
                assert!(
                    !focused || view.sections.selected == Some(ids[0]),
                    "focus retains its section driver while range scope includes hidden tracks"
                );
            });
            assert!(
                !visual.update(|window, app| editor.read(app).section_has_focus(window)),
                "range owns keyboard actions while Q keeps its display driver"
            );
            visual.simulate_keystrokes("cmd-c");
            editor.read_with(cx, |view, _| {
                assert_eq!(view.last_time_request, Some(("copy", None)))
            });
        }
    }
    #[gpui::test]
    fn section_has_opaque_chamfered_fill_and_compact_header_across_palettes_and_layouts(
        cx: &mut TestAppContext,
    ) {
        struct RestoreTheme(hex_preferences::ThemePreferences);
        impl Drop for RestoreTheme {
            fn drop(&mut self) {
                theme::apply_preferences(self.0);
            }
        }
        let _restore = RestoreTheme(theme::preferences());
        let (handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        for (width, palette) in [
            (880.0, hex_preferences::ThemePreferences::default()),
            (
                1280.0,
                hex_preferences::ThemePreferences {
                    background: hex_preferences::RgbColor::new(225, 229, 238),
                    text: hex_preferences::RgbColor::new(24, 27, 34),
                    primary: hex_preferences::RgbColor::new(145, 25, 45),
                    secondary: hex_preferences::RgbColor::new(16, 70, 145),
                },
            ),
        ] {
            theme::apply_preferences(palette);
            editor.update(cx, |view, cx| {
                view.viewport = Some(Viewport {
                    start: 24_000,
                    length: 240_000,
                });
                cx.notify();
            });
            visual.simulate_resize(gpui::size(px(width), px(600.0)));
            visual.simulate_mouse_move(point(px(4.0), px(4.0)), None, Modifiers::default());
            visual.run_until_parked();
            let body = bounds(&mut visual, 0);
            let header = visual
                .debug_bounds("arrangement-section-header-0")
                .expect("actual section header");
            assert!((f32::from(header.top() - body.top())).abs() <= 1.0);
            assert!(
                (f32::from(header.size.height) - super::super::clip_name::CLIP_NAME_HEIGHT).abs()
                    <= 0.1,
                "header remains a fixed compact height"
            );
            assert!(header.right() <= body.right() + px(1.0));
            let (fill, _, _) = theme::section_colors(ElementColor::Blue, false, false);
            assert_eq!(
                painted_fill(&mut visual, 0),
                Some(rgb(fill).into()),
                "opaque tier fill"
            );
            let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
            let fills: Vec<_> = visual
                .rendered_filled_quads("arrangement-section-0")
                .into_iter()
                .filter(|(_, c)| *c == gpui::Background::from(rgb(fill)))
                .collect();
            assert!(
                fills.iter().any(
                    |(b, _)| (b.bottom() / scale - f32::from(body.bottom())).abs() < 0.1
                        && b.right() / scale < f32::from(body.right()) - 1.0
                ),
                "cut leaves the bottom-right corner unfilled"
            );
            for (at, cursor) in [
                (header.center(), gpui::CursorStyle::OpenHand),
                (
                    point(body.center().x, header.bottom() + px(1.0)),
                    gpui::CursorStyle::Arrow,
                ),
                (
                    point(body.right() - px(1.0), body.top() + body.size.height * 0.75),
                    gpui::CursorStyle::ResizeRightBracket,
                ),
            ] {
                visual.simulate_mouse_move(at, None, Modifiers::default());
                visual.run_until_parked();
                assert_eq!(visual.rendered_cursor_style(), Some(cursor));
            }
            editor.read_with(cx, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project, before)
            });
        }
    }
    #[gpui::test]
    fn shift_click_selects_section_blocks_in_header_and_body_without_time_ranges(
        cx: &mut TestAppContext,
    ) {
        for (focused, body) in [(false, false), (false, true), (true, false), (true, true)] {
            let (handle, editor, ids) = fixture(cx);
            editor.update(cx, |view, cx| {
                for command in [
                    Command::AddTrack {
                        name: "hidden empty track".into(),
                    },
                    Command::SetTempoPoint {
                        beat: BeatTime::ONE,
                        tempo: hex_project::Tempo::new(180_000_000).unwrap(),
                    },
                    Command::SetArrangementSectionRange {
                        section: ids[0],
                        start: BeatTime::from_ratio(1, 7).unwrap(),
                        end: BeatTime::from_ratio(27, 7).unwrap(),
                    },
                ] {
                    let request = if matches!(command, Command::SetArrangementSectionRange { .. }) {
                        Request::EditArrangementSection {
                            project_generation: view.project_generation,
                            revision: view.state.as_ref().unwrap().revision,
                            command,
                        }
                    } else {
                        Request::Edit(command)
                    };
                    assert!(view.enqueue(request));
                    let event = next(&view.service);
                    assert!(
                        event.error.is_none(),
                        "fixture edit refused: {:?}",
                        event.error
                    );
                    view.receive(event);
                }
                view.section_autoplay = false;
                cx.notify();
            });
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            let header = editor.read_with(cx, |view, _| position(view, 10));
            visual.simulate_mouse_down(header, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(header, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            if focused {
                visual.simulate_keystrokes("q");
                visual.run_until_parked();
            }
            editor.update(cx, |view, cx| {
                let first = view.state.as_ref().unwrap().project.tracks()[0].id();
                view.time_selection = Some(super::super::time_selection::TimeSelection {
                    anchor_track: Some(first),
                    end_track: Some(first),
                    anchor_frame: 111,
                    end_frame: 222,
                });
                view.time_selection_project_scope = false;
                view.section_autoplay = true;
                view.last_enqueued_edit = None;
                cx.notify();
            });
            visual.run_until_parked();
            let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().clone());
            let at = editor.read_with(cx, |view, _| {
                let [_, top, _, height] = view.section_layout().unwrap();
                point(
                    position(view, 2).x,
                    if body {
                        px(top + height * 0.75)
                    } else {
                        header.y
                    },
                )
            });
            let shift = Modifiers {
                shift: true,
                ..Modifiers::default()
            };
            visual.simulate_mouse_down(at, MouseButton::Left, shift);
            visual.simulate_mouse_up(at, MouseButton::Left, shift);
            visual.run_until_parked();
            editor.read_with(cx, |view, _| {
                assert!(view.time_selection.is_none());
                assert!(!view.time_selection_project_scope);
                assert_eq!(view.sections.selection, vec![ids[0], ids[1]]);
                assert_eq!(view.sections.selected, Some(ids[0]));
                assert!(view.selected_clip_ids().is_empty());
                assert!(
                    view.sections.gesture.is_none()
                        && view.sections.pending.is_none()
                        && view.sections.cursor.is_none()
                );
                assert!(view.time_selection_gesture.is_none());
                assert!(view.last_enqueued_edit.is_none());
                assert!(
                    view.pending_audio_commands.is_empty()
                        && view.prepared_seek.is_none()
                        && view.latched_seek_intent.is_none()
                );
                assert_eq!(view.worker_requests_pending, 0);
                assert_eq!(view.state.as_ref().unwrap().project, before.project);
                assert_eq!(view.state.as_ref().unwrap().revision, before.revision);
                assert_eq!(
                    (
                        view.state.as_ref().unwrap().can_undo,
                        view.state.as_ref().unwrap().can_redo
                    ),
                    (before.can_undo, before.can_redo)
                );
                assert_eq!(view.arrangement_focus.projection.enabled, focused);
            });
            let toggle = Modifiers {
                shift: true,
                platform: true,
                ..Modifiers::default()
            };
            visual.simulate_mouse_down(at, MouseButton::Left, toggle);
            visual.simulate_mouse_up(at, MouseButton::Left, toggle);
            visual.run_until_parked();
            editor.read_with(cx, |view, _| {
                assert_eq!(view.sections.selection, vec![ids[1]]);
                assert_eq!(view.sections.selected, Some(ids[1]));
                assert!(view.time_selection.is_none());
            });
        }
    }
    #[gpui::test]
    fn shift_click_selects_every_section_between_anchor_and_click(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut all = vec![ids[0]];
        editor.update(cx, |view, cx| {
            for (start, end, name) in [(4, 8, "Verse"), (12, 16, "Bridge")] {
                assert!(view.enqueue(Request::EditArrangementSection {
                    project_generation: view.project_generation,
                    revision: view.state.as_ref().unwrap().revision,
                    command: Command::AddArrangementSection {
                        name: name.into(),
                        color: ElementColor::Green,
                        start: BeatTime::from_quarters(start).unwrap(),
                        end: BeatTime::from_quarters(end).unwrap(),
                    },
                }));
                let event = next(&view.service);
                assert!(event.error.is_none(), "{:?}", event.error);
                let Some(Created::ArrangementSection(id)) = event.created else {
                    panic!("section result")
                };
                all.push(id);
                view.receive(event);
            }
            view.section_autoplay = false;
            cx.notify();
        });
        // Song order: intro, verse, chorus, bridge.
        let all = [all[0], all[1], ids[1], all[2]];
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let mut click = |quarters, modifiers| {
            let at = editor.read_with(&visual, |view, _| position(view, quarters));
            visual.simulate_mouse_down(at, MouseButton::Left, modifiers);
            visual.simulate_mouse_up(at, MouseButton::Left, modifiers);
            visual.run_until_parked();
            editor.read_with(&visual, |view, _| {
                assert!(view.time_selection.is_none());
                (
                    view.sections.selected_ids().to_vec(),
                    view.sections.selected,
                )
            })
        };
        click(2, Modifiers::default());
        assert_eq!(click(14, shift), (all.to_vec(), Some(all[3])));
        // Same anchor: a shorter range replaces the previous one.
        assert_eq!(click(10, shift), (all[..3].to_vec(), Some(all[2])));
        click(14, Modifiers::default());
        assert_eq!(click(2, shift), (all.to_vec(), Some(all[0])));
    }
    #[gpui::test]
    fn menu_merge_click_over_the_lane_merges_and_undo_restores(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let (a, b) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 10)));
        let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.clone());
        visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        // One selected section: g explains instead of silently consuming input.
        visual.simulate_keystrokes("g");
        editor.read_with(cx, |v, _| {
            assert!(v.sections.pending.is_none());
            assert!(
                v.error
                    .as_ref()
                    .is_some_and(|e| e.contains("shift-click another section to merge a span"))
            );
        });
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(b, MouseButton::Left, shift);
        visual.simulate_mouse_up(b, MouseButton::Left, shift);
        visual.run_until_parked();
        let body = editor.read_with(cx, |v, _| {
            let [_, top, _, height] = v.section_layout().unwrap();
            point(a.x, px(top + height * 0.75))
        });
        visual.simulate_mouse_down(body, MouseButton::Right, Modifiers::default());
        visual.simulate_mouse_up(body, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert_eq!(
                v.context_menu.as_ref().unwrap().items[0].label,
                "merge selected span · g"
            )
        });
        // The first row opens at the pointer, above the lane's own capture.
        let panel = visual.debug_bounds("context-menu-panel").unwrap();
        let row = point(panel.origin.x + px(20.0), panel.origin.y + px(14.0));
        visual.simulate_mouse_down(row, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(row, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let event = editor.update(cx, |v, _| {
            assert!(v.context_menu.is_none());
            assert!(matches!(
                v.sections.pending.as_ref().map(|p| &p.command),
                Some(Command::MergeArrangementSections(_))
            ));
            next(&v.service)
        });
        assert!(event.error.is_none(), "{:?}", event.error);
        editor.update(cx, |v, cx| {
            v.receive(event);
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.arrangement_sections().len(), 1);
            assert_eq!(
                p.arrangement_section(ids[0]).unwrap().end(),
                BeatTime::from_quarters(12).unwrap()
            );
            assert_eq!(p.tracks(), before.tracks());
            cx.notify();
        });
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-z");
        let event = editor.update(cx, |v, _| next(&v.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        editor.update(cx, |v, _| {
            v.receive(event);
            assert_eq!(v.state.as_ref().unwrap().project, before);
        });
    }
    /// Tick the real worker pump until every edit and queued follow-up lands.
    fn settle(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            let idle = editor.update(visual, |v, cx| {
                v.tick(cx);
                !v.edit_pending() && v.sections.pending.is_none() && v.sections.queued.is_none()
            });
            if idle {
                break;
            }
            assert!(Instant::now() < deadline, "section edits settle");
            std::thread::sleep(Duration::from_millis(2));
        }
        visual.run_until_parked();
    }
    #[gpui::test]
    fn section_copy_paints_committed_rows_before_the_terminal(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let source = editor.read_with(cx, |view, _| position(view, 2));
        visual.simulate_click(source, Modifiers::default());
        visual.simulate_keystrokes("q");
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-d");

        let terminal = editor.read_with(cx, |view, _| next(&view.service));
        let observed = editor.read_with(cx, |view, _| {
            view.service.try_arrangement_edit_view().unwrap()
        });
        let Some(Created::ArrangementSection(copy)) = terminal.created else {
            panic!("section copy");
        };
        assert!(matches!(
            observed.kind,
            hex_session::service::ArrangementEditViewKind::Duplicate(Created::ArrangementSection(id))
                if id == copy
        ));
        let committed = observed.state.upgrade().unwrap();
        let marker = committed.project.arrangement_section(copy).unwrap();
        let midi = committed.project.tracks()[0]
            .midi_clips()
            .iter()
            .find(|clip| clip.start() == marker.start())
            .unwrap()
            .id();
        let selector = Box::leak(format!("midi-clip-visual-{}", midi.value()).into_boxed_str());
        visual.run_until_parked();
        assert!(
            visual.debug_bounds(selector).is_none(),
            "the old focus cache has no copy"
        );

        editor.update(cx, |view, cx| {
            assert!(view.arrangement_focus.projection.enabled);
            assert_eq!(view.sections.selected, Some(ids[0]));
            for (generation, revision) in [
                (observed.project_generation + 1, observed.previous_revision),
                (observed.project_generation, observed.previous_revision + 1),
            ] {
                assert!(
                    !view.observe_duplicate_view(ArrangementEditViewObservation {
                        project_generation: generation,
                        first_previous_revision: revision,
                        previous_revision: revision,
                        kind: observed.kind,
                        state: Arc::downgrade(&committed),
                    })
                );
            }
            assert!(view.observe_duplicate_view(observed));
            assert!(view.section_install_pending());
            assert!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_section(copy)
                    .is_none()
            );
            assert!(Arc::ptr_eq(
                &view.arrangement_rows_state().unwrap(),
                &committed
            ));
            cx.notify();
        });
        visual.run_until_parked();
        let before_ack = visual
            .debug_bounds(selector)
            .expect("committed copy is visible");
        let marker_bounds = visual.debug_bounds("arrangement-section-1").unwrap();
        editor.update(cx, |view, cx| {
            view.receive(terminal);
            assert!(view.committed_section_copy_view().is_none());
            assert!(!view.section_install_pending());
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(selector), Some(before_ack));
        assert_eq!(
            visual.debug_bounds("arrangement-section-1"),
            Some(marker_bounds)
        );
    }

    #[gpui::test]
    fn consecutive_keyboard_section_copies_keep_both_receipts_and_latest_rows(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, _ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let source = editor.read_with(cx, |view, _| position(view, 2));
        visual.simulate_click(source, Modifiers::default());
        visual.simulate_keystrokes("q");
        visual.run_until_parked();
        let initial = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
        visual.simulate_keystrokes("cmd-d cmd-d");
        let deadline = crate::test_wait::hang_deadline();
        let first_view = loop {
            if let Ok(view) =
                editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
            {
                break view;
            }
            assert!(Instant::now() < deadline, "first committed section view");
            std::thread::yield_now();
        };
        let mut first_terminal = editor.read_with(&visual, |view, _| next(&view.service));
        let first_state = first_view.state.upgrade().unwrap();
        editor.update(&mut visual, |view, cx| {
            // A refused, still-queued successor must not retire its predecessor.
            let primary = view.sections.pending.as_ref().unwrap();
            let command = primary.command.clone();
            view.sections.ahead = Some(Pending {
                generation: primary.generation,
                revision: initial + 1,
                command: command.clone(),
                display: primary.display.clone(),
                layout: None,
                copied: None,
                receipt: Receipt::new(primary.generation),
                selection_intent_epoch: primary.selection_intent_epoch,
            });
            assert!(view.refuse_section(&Request::EditArrangementSection {
                project_generation: view.project_generation,
                revision: initial + 1,
                command,
            }));
            assert!(view.sections.pending.is_some());
            assert!(view.sections.ahead.is_none());
            assert!(view.observe_duplicate_view(first_view));
            assert!(view.sections.pending.is_some());
            assert!(
                view.sections.ahead.is_some(),
                "successor admitted before first terminal"
            );
            cx.notify();
        });
        visual.run_until_parked();
        let second_view = loop {
            if let Ok(view) =
                editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
            {
                break view;
            }
            assert!(Instant::now() < deadline, "second committed section view");
            std::thread::yield_now();
        };
        let second_terminal = editor.read_with(&visual, |view, _| next(&view.service));
        assert!(first_terminal.error.is_none());
        assert!(second_terminal.error.is_none());
        let second_state = second_view.state.upgrade().unwrap();
        assert_eq!(second_state.revision, initial + 2);
        let Some(Created::ArrangementSection(copy)) = second_terminal.created else {
            panic!("second section copy");
        };
        let copy_start = second_state
            .project
            .arrangement_section(copy)
            .unwrap()
            .start();
        let midi = second_state.project.tracks()[0]
            .midi_clips()
            .iter()
            .find(|clip| {
                clip.start() == copy_start && first_state.project.midi_clip(clip.id()).is_none()
            })
            .unwrap()
            .id();
        let selector = Box::leak(format!("midi-clip-visual-{}", midi.value()).into_boxed_str());
        editor.update(&mut visual, |view, cx| {
            for (generation, revision) in [
                (
                    second_view.project_generation + 1,
                    second_view.previous_revision,
                ),
                (
                    second_view.project_generation,
                    second_view.previous_revision + 1,
                ),
            ] {
                assert!(
                    !view.observe_duplicate_view(ArrangementEditViewObservation {
                        project_generation: generation,
                        first_previous_revision: revision,
                        previous_revision: revision,
                        kind: second_view.kind,
                        state: Arc::downgrade(&second_state),
                    })
                );
            }
            assert!(view.observe_duplicate_view(second_view));
            assert!(Arc::ptr_eq(
                &view.arrangement_rows_state().unwrap(),
                &second_state
            ));
            cx.notify();
        });
        visual.run_until_parked();
        let before = visual
            .debug_bounds(selector)
            .expect("second committed copy paints");
        let marker = visual.debug_bounds("arrangement-section-2").unwrap();
        editor.update(&mut visual, |view, cx| {
            view.history_action(Request::Undo, cx)
        });
        first_terminal.error = Some("automatic recovery: test warning".into());
        editor.update(&mut visual, |view, cx| {
            view.receive(first_terminal);
            assert!(Arc::ptr_eq(
                &view.arrangement_rows_state().unwrap(),
                &second_state
            ));
            assert!(
                view.sections.pending.is_some(),
                "warning cannot retire an admitted successor"
            );
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(selector), Some(before));
        assert_eq!(visual.debug_bounds("arrangement-section-2"), Some(marker));
        editor.update(&mut visual, |view, cx| {
            view.receive(second_terminal);
            assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &second_state));
            assert!(view.sections.pending.is_none());
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(selector), Some(before));
        assert_eq!(visual.debug_bounds("arrangement-section-2"), Some(marker));
        settle(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().revision, initial + 3);
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_sections()
                    .len(),
                first_state.project.arrangement_sections().len(),
                "undo follows both admitted copies"
            );
        });
    }

    #[gpui::test]
    fn unrelated_worker_completion_preserves_existing_status(cx: &mut TestAppContext) {
        let (_handle, editor, _ids) = fixture(cx);
        editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddTrack {
                name: "other".into()
            })));
        });
        let mut event = editor.read_with(cx, |view, _| next(&view.service));
        assert!(event.error.is_none());
        event.status = None;
        editor.update(cx, |view, _| {
            view.status = "existing status".into();
            view.receive(event);
            assert_eq!(&*view.status, "existing status");
        });
    }

    #[gpui::test]
    fn newer_selection_does_not_admit_an_ahead_section_copy(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |view, _| {
            view.select_section_metadata(Some(ids[0]));
            view.section_metadata(Command::DuplicateArrangementSection(ids[0]));
            view.section_metadata(Command::DuplicateArrangementSection(ids[0]));
        });
        let deadline = crate::test_wait::hang_deadline();
        let first = loop {
            if let Ok(view) =
                editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
            {
                break view;
            }
            assert!(Instant::now() < deadline, "first section view");
            std::thread::yield_now();
        };
        let terminal = editor.read_with(&visual, |view, _| next(&view.service));
        editor.update(&mut visual, |view, _| {
            view.select_section_metadata(Some(ids[1]));
            assert!(view.observe_duplicate_view(first));
            assert!(
                view.sections.ahead.is_none(),
                "newer selection must not admit a successor"
            );
            view.receive(terminal);
            assert_eq!(view.sections.selected, Some(ids[1]));
            // The already requested command remains in the ordinary queue.
            assert!(view.sections.queued.is_some());
        });
    }

    #[gpui::test]
    fn project_replacement_retires_both_section_receipts(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |view, _| {
            view.select_section_metadata(Some(ids[0]));
            view.section_metadata(Command::DuplicateArrangementSection(ids[0]));
            view.section_metadata(Command::DuplicateArrangementSection(ids[0]));
        });
        let deadline = crate::test_wait::hang_deadline();
        let first = loop {
            if let Ok(view) =
                editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
            {
                break view;
            }
            assert!(Instant::now() < deadline, "first section view");
            std::thread::yield_now();
        };
        let _first_terminal = editor.read_with(&visual, |view, _| next(&view.service));
        editor.update(&mut visual, |view, _| {
            assert!(view.observe_duplicate_view(first))
        });
        let second = loop {
            if let Ok(view) =
                editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
            {
                break view;
            }
            assert!(Instant::now() < deadline, "second section view");
            std::thread::yield_now();
        };
        let _second_terminal = editor.read_with(&visual, |view, _| next(&view.service));
        let stale_state = second.state.upgrade().unwrap();
        let stale_kind = second.kind;
        let stale_generation = second.project_generation;
        let stale_revision = second.previous_revision;
        editor.update(&mut visual, |view, _| {
            assert!(view.observe_duplicate_view(second));
            assert!(view.sections.pending.is_some() && view.sections.ahead.is_some());
            view.service
                .submit(Request::NewWithAudioMode {
                    mode: hex_session::DocumentAudioMode::Resident,
                    reservation: None,
                })
                .unwrap();
        });
        let replacement = editor.read_with(&visual, |view, _| next(&view.service));
        assert!(replacement.project_replaced && replacement.error.is_none());
        editor.update(&mut visual, |view, _| {
            view.receive(replacement);
            assert!(view.sections.pending.is_none() && view.sections.ahead.is_none());
            assert!(
                !view.observe_duplicate_view(ArrangementEditViewObservation {
                    project_generation: stale_generation,
                    first_previous_revision: stale_revision,
                    previous_revision: stale_revision,
                    kind: stale_kind,
                    state: Arc::downgrade(&stale_state),
                })
            );
        });
    }

    #[gpui::test]
    fn play_uses_applied_successor_while_both_section_receipts_wait(cx: &mut TestAppContext) {
        use std::sync::atomic::Ordering::Acquire;

        let commands = vec![
            Command::AddTrack {
                name: "track".into(),
            },
            Command::AddArrangementSection {
                name: "section".into(),
                color: ElementColor::Blue,
                start: BeatTime::from_quarters(8).unwrap(),
                end: BeatTime::from_quarters(12).unwrap(),
            },
        ];
        let mut fixture =
            super::super::worker_ui_fixture::open_with_section_support(cx, commands, false, true);
        let editor = fixture.editor.clone();
        let mut visual = VisualTestContext::from_window(fixture.handle, cx);
        visual.run_until_parked();
        let (revision, expected) = editor.update(&mut visual, |view, _| {
            let section = view.state.as_ref().unwrap().project.arrangement_sections()[0].id();
            view.select_section_metadata(Some(section));
            view.play_start_at_beginning = false;
            let expected = view.playback_start_frame();
            let revision = view.state.as_ref().unwrap().revision;
            view.section_metadata(Command::DuplicateArrangementSection(section));
            view.section_metadata(Command::DuplicateArrangementSection(section));
            (revision, expected)
        });
        let deadline = crate::test_wait::hang_deadline();
        let first = loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if let Ok(view) =
                editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
            {
                break view;
            }
            assert!(Instant::now() < deadline, "first section view");
            std::thread::yield_now();
        };
        let first_terminal = loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
                break event;
            }
            assert!(Instant::now() < deadline, "first section terminal");
            std::thread::yield_now();
        };
        assert!(first_terminal.error.is_none(), "{:?}", first_terminal.error);
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            let ready = editor.read_with(&visual, |view, _| {
                view.audio_status().is_some_and(|status| {
                    !status.stopped_edit_pending()
                        && status
                            .recording_source()
                            .is_some_and(|source| source.context.editor_revision == revision + 1)
                })
            });
            if ready {
                break;
            }
            assert!(Instant::now() < deadline, "first source and released fence");
            std::thread::yield_now();
        }
        editor.update(&mut visual, |view, cx| {
            assert!(view.observe_duplicate_view(first));
            assert!(view.sections.ahead.is_some());
            cx.notify();
        });
        let second = loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if let Ok(view) =
                editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
            {
                break view;
            }
            assert!(Instant::now() < deadline, "second section view");
            std::thread::yield_now();
        };
        let second_terminal = loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
                break event;
            }
            assert!(Instant::now() < deadline, "second section terminal");
            std::thread::yield_now();
        };
        assert!(
            second_terminal.error.is_none(),
            "{:?}",
            second_terminal.error
        );
        let committed = second.state.upgrade().unwrap();
        editor.update(&mut visual, |view, cx| {
            assert!(view.observe_duplicate_view(second));
            cx.notify();
        });
        visual.run_until_parked();
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            let ready = editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                view.section_copy_play_source()
                    .is_some_and(|source| source.context.editor_revision == revision + 2)
            });
            if ready {
                break;
            }
            assert!(Instant::now() < deadline, "applied successor source");
            std::thread::yield_now();
        }
        editor.update(&mut visual, |view, cx| {
            assert!(Arc::ptr_eq(
                &view.arrangement_rows_state().unwrap(),
                &committed
            ));
            view.toggle_play();
            assert_eq!(
                view.last_transport_command,
                Some(hex_engine::playback::PlaybackCommand::PlayFrom(expected))
            );
            cx.notify();
        });
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        editor.read_with(&visual, |view, _| {
            assert!(view.audio_status().unwrap().playing.load(Acquire))
        });
        editor.update(&mut visual, |view, cx| {
            view.receive(first_terminal);
            assert!(Arc::ptr_eq(
                &view.arrangement_rows_state().unwrap(),
                &committed
            ));
            view.receive(second_terminal);
            assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &committed));
            cx.notify();
        });
        fixture.finish(cx);
    }

    #[gpui::test]
    fn held_recovery_allows_next_section_copy_and_applied_play(cx: &mut TestAppContext) {
        use std::sync::atomic::Ordering::Acquire;

        let environment = super::super::test_environment::Environment::new().unwrap();
        let recovery = environment.root().join("held-section.hex-journal");
        let commands = vec![
            Command::AddTrack {
                name: "track".into(),
            },
            Command::AddArrangementSection {
                name: "section".into(),
                color: ElementColor::Blue,
                start: BeatTime::from_quarters(8).unwrap(),
                end: BeatTime::from_quarters(12).unwrap(),
            },
        ];
        let mut fixture = super::super::worker_ui_fixture::open_with_section_and_recovery(
            cx,
            commands,
            false,
            true,
            Some(recovery.clone()),
        );
        let editor = fixture.editor.clone();
        let mut visual = VisualTestContext::from_window(fixture.handle, cx);
        visual.run_until_parked();
        let mut gate = hex_session::service::hold_next_checkpoint(recovery).unwrap();
        let (revision, expected) = editor.update(&mut visual, |view, _| {
            let section = view.state.as_ref().unwrap().project.arrangement_sections()[0].id();
            view.select_section_metadata(Some(section));
            view.play_start_at_beginning = false;
            let expected = view.playback_start_frame();
            let revision = view.state.as_ref().unwrap().revision;
            view.section_metadata(Command::DuplicateArrangementSection(section));
            view.section_metadata(Command::DuplicateArrangementSection(section));
            (revision, expected)
        });
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut entered_at = None;
        let mut first = None;
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if entered_at.is_none() {
                match gate.try_entered() {
                    Ok(()) => entered_at = Some(Instant::now()),
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(error) => panic!("checkpoint gate failed: {error}"),
                }
            }
            if first.is_none() {
                first = editor.read_with(&visual, |view, _| {
                    view.service.try_arrangement_edit_view().ok()
                });
            }
            let applied = editor.read_with(&visual, |view, _| {
                view.audio_status().is_some_and(|status| {
                    !status.stopped_edit_pending()
                        && status
                            .recording_source()
                            .is_some_and(|source| source.context.editor_revision == revision + 1)
                })
            });
            if entered_at.is_some() && first.is_some() && applied {
                break;
            }
            assert!(
                entered_at.is_none_or(|time| time.elapsed() < Duration::from_millis(1500)),
                "first source did not arrive within the held checkpoint"
            );
            assert!(
                Instant::now() < deadline,
                "held first checkpoint and applied source"
            );
            std::thread::yield_now();
        }
        editor.update(&mut visual, |view, cx| {
            assert!(
                view.service.try_event().is_err(),
                "first terminal waits for recovery"
            );
            assert!(view.observe_duplicate_view(first.take().unwrap()));
            assert!(view.sections.ahead.is_some());
            cx.notify();
        });
        let second = loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if let Ok(view) =
                editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
            {
                break view;
            }
            assert!(
                entered_at.is_some_and(|time| time.elapsed() < Duration::from_millis(1500)),
                "second copy missed the held checkpoint"
            );
            assert!(
                Instant::now() < deadline,
                "second copy while recovery is held"
            );
            std::thread::yield_now();
        };
        let committed = second.state.upgrade().unwrap();
        assert_eq!(committed.revision, revision + 2);
        editor.update(&mut visual, |view, cx| {
            assert!(
                view.service.try_event().is_err(),
                "ordered terminals still wait for recovery"
            );
            assert!(view.observe_duplicate_view(second));
            cx.notify();
        });
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            let ready = editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                view.section_copy_play_source()
                    .is_some_and(|source| source.context.editor_revision == revision + 2)
            });
            if ready {
                break;
            }
            assert!(
                entered_at.is_some_and(|time| time.elapsed() < Duration::from_millis(1500)),
                "applied source missed the held checkpoint"
            );
            assert!(
                Instant::now() < deadline,
                "applied second source while recovery is held"
            );
            std::thread::yield_now();
        }
        editor.update(&mut visual, |view, cx| {
            view.toggle_play();
            assert_eq!(
                view.last_transport_command,
                Some(hex_engine::playback::PlaybackCommand::PlayFrom(expected))
            );
            cx.notify();
        });
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        editor.read_with(&visual, |view, _| {
            assert!(view.audio_status().unwrap().playing.load(Acquire))
        });
        visual.run_until_parked();
        let marker = visual.debug_bounds("arrangement-section-1").unwrap();
        gate.release()
            .expect("checkpoint must still be held after the second copy and Play");
        let first_terminal = loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
                break event;
            }
            assert!(Instant::now() < deadline, "first durable terminal");
            std::thread::yield_now();
        };
        let second_terminal = loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
                break event;
            }
            assert!(Instant::now() < deadline, "second durable terminal");
            std::thread::yield_now();
        };
        assert!(first_terminal.error.is_none() && second_terminal.error.is_none());
        editor.update(&mut visual, |view, cx| {
            view.receive(first_terminal);
            assert!(Arc::ptr_eq(
                &view.arrangement_rows_state().unwrap(),
                &committed
            ));
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds("arrangement-section-1"), Some(marker));
        editor.update(&mut visual, |view, cx| {
            view.receive(second_terminal);
            assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &committed));
            view.toggle_play();
            cx.notify();
        });
        fixture.backend.render(&mut [[0.0; 2]; 512]).unwrap();
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds("arrangement-section-1"), Some(marker));
        editor.update(&mut visual, |view, cx| {
            view.history_action(Request::Undo, cx)
        });
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            let undone = editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                view.state
                    .as_ref()
                    .is_some_and(|state| state.revision == revision + 3)
            });
            if undone {
                break;
            }
            assert!(Instant::now() < deadline, "undo after durable copies");
            std::thread::yield_now();
        }
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_sections()
                    .len(),
                2
            )
        });
        fixture.finish(cx);
        drop(environment);
    }

    #[gpui::test]
    fn applied_pointer_section_copy_plays_from_the_selected_copy(cx: &mut TestAppContext) {
        pointer_section_copy_plays_from_the_selected_copy(cx, 0);
    }

    #[gpui::test]
    fn appended_section_copy_with_gap_plays_from_the_selected_copy(cx: &mut TestAppContext) {
        pointer_section_copy_plays_from_the_selected_copy(cx, 1);
    }

    #[gpui::test]
    fn replaced_section_copy_plays_from_the_selected_copy(cx: &mut TestAppContext) {
        pointer_section_copy_plays_from_the_selected_copy(cx, 2);
    }

    fn pointer_section_copy_plays_from_the_selected_copy(cx: &mut TestAppContext, copy_kind: u8) {
        let environment = super::super::test_environment::Environment::new().unwrap();
        let recovery = environment.root().join("held-pointer-section.hex-journal");
        let mut commands = vec![
            Command::AddTrack {
                name: "track".into(),
            },
            Command::AddArrangementSection {
                name: "section".into(),
                color: ElementColor::Blue,
                start: BeatTime::from_quarters(8).unwrap(),
                end: BeatTime::from_quarters(12).unwrap(),
            },
        ];
        if copy_kind == 2 {
            commands.push(Command::AddArrangementSection {
                name: "target".into(),
                color: ElementColor::Blue,
                start: BeatTime::from_quarters(12).unwrap(),
                end: BeatTime::from_quarters(16).unwrap(),
            });
        }
        let mut fixture = super::super::worker_ui_fixture::open_with_section_and_recovery(
            cx,
            commands,
            false,
            true,
            Some(recovery.clone()),
        );
        let editor = fixture.editor.clone();
        let mut visual = VisualTestContext::from_window(fixture.handle, cx);
        visual.run_until_parked();
        let mut gate = hex_session::service::hold_next_checkpoint(recovery).unwrap();
        let original_revision = editor.update(&mut visual, |view, _| {
            let section = view.state.as_ref().unwrap().project.arrangement_sections()[0].id();
            view.select_section_metadata(Some(section));
            view.play_start_at_beginning = false;
            let revision = view.state.as_ref().unwrap().revision;
            let command = if copy_kind == 1 {
                Command::AppendArrangementSectionCopy {
                    section,
                    start: BeatTime::from_quarters(16).unwrap(),
                    gap_name: "gap".into(),
                    gap_color: ElementColor::Blue,
                }
            } else if copy_kind == 2 {
                let target = view.state.as_ref().unwrap().project.arrangement_sections()[1].id();
                Command::ReplaceArrangementSection {
                    source: section,
                    target,
                    copy: true,
                }
            } else {
                Command::DuplicateArrangementSectionAt {
                    section,
                    target: section,
                    after: true,
                }
            };
            view.section_metadata(command);
            revision
        });
        let deadline = crate::test_wait::hang_deadline();
        let observation = loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if let Ok(observation) =
                editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
            {
                break observation;
            }
            assert!(Instant::now() < deadline, "committed section view");
            std::thread::yield_now();
        };
        let committed = observation.state.upgrade().unwrap();
        loop {
            match gate.try_entered() {
                Ok(()) => break,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    assert!(Instant::now() < deadline, "checkpoint was not held");
                    std::thread::yield_now();
                }
                Err(error) => panic!("checkpoint gate failed: {error}"),
            }
        }
        let copied = match observation.kind {
            hex_session::service::ArrangementEditViewKind::Duplicate(
                hex_project::Created::ArrangementSection(id),
            ) => id,
            _ => panic!("unexpected committed view"),
        };
        let expected = committed
            .project
            .tempo_map()
            .beat_to_frame(
                committed
                    .project
                    .arrangement_section(copied)
                    .unwrap()
                    .start(),
            )
            .unwrap();
        editor.update(&mut visual, |view, cx| {
            assert!(view.observe_duplicate_view(observation));
            assert_eq!(view.state.as_ref().unwrap().revision, original_revision);
            cx.notify();
        });
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            let applied = editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                view.audio_status()
                    .unwrap()
                    .recording_source()
                    .is_some_and(|source| source.context.editor_revision == original_revision + 1)
            });
            if applied {
                break;
            }
            assert!(Instant::now() < deadline, "applied copied section source");
            std::thread::yield_now();
        }
        editor.update(&mut visual, |view, cx| {
            assert_eq!(view.state.as_ref().unwrap().revision, original_revision);
            assert!(
                view.service.try_event().is_err(),
                "terminal must still be held"
            );
            let observation = view.pending_publication_observation.as_mut().unwrap();
            observation.output_identity += 1;
            assert!(
                view.section_copy_play_source().is_none(),
                "wrong output must refuse"
            );
            view.pending_publication_observation
                .as_mut()
                .unwrap()
                .output_identity -= 1;
            let epoch = view.selection_intent_epoch;
            view.advance_selection_intent();
            assert!(
                view.section_copy_play_source().is_none(),
                "changed selection must refuse"
            );
            view.selection_intent_epoch = epoch;
            view.play_start_at_beginning = true;
            assert_eq!(view.section_copy_play_start(), Some(0));
            view.play_start_at_beginning = false;
            view.toggle_play();
            assert_eq!(
                view.last_transport_command,
                Some(hex_engine::playback::PlaybackCommand::PlayFrom(expected))
            );
            cx.notify();
        });
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        editor.read_with(&visual, |view, _| {
            assert!(
                view.audio_status()
                    .unwrap()
                    .playing
                    .load(std::sync::atomic::Ordering::Acquire)
            );
        });
        gate.release()
            .expect("checkpoint remains held until after Play");
        let terminal = loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
                break event;
            }
            assert!(Instant::now() < deadline, "section terminal");
            std::thread::yield_now();
        };
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        editor.update(&mut visual, |view, cx| {
            view.receive(terminal);
            assert_eq!(view.playback_start_frame(), expected);
            cx.notify();
        });
        fixture.finish(cx);
    }

    #[gpui::test]
    fn applied_keyboard_section_copy_starts_before_its_terminal(cx: &mut TestAppContext) {
        use std::sync::atomic::Ordering::Acquire;

        let commands = vec![
            Command::AddTrack {
                name: "track".into(),
            },
            Command::AddArrangementSection {
                name: "section".into(),
                color: ElementColor::Blue,
                start: BeatTime::from_quarters(8).unwrap(),
                end: BeatTime::from_quarters(12).unwrap(),
            },
        ];
        let mut fixture =
            super::super::worker_ui_fixture::open_with_section_support(cx, commands, false, true);
        let editor = fixture.editor.clone();
        let mut visual = VisualTestContext::from_window(fixture.handle, cx);
        visual.run_until_parked();
        let (source_revision, expected) = editor.update(&mut visual, |view, cx| {
            let section = view.state.as_ref().unwrap().project.arrangement_sections()[0].id();
            view.select_section_metadata(Some(section));
            view.play_start_at_beginning = false;
            let expected = view.playback_start_frame();
            let revision = view.state.as_ref().unwrap().revision;
            view.section_metadata(Command::DuplicateArrangementSection(section));
            view.toggle_play();
            assert!(
                view.last_transport_command.is_none(),
                "unapplied source cannot start"
            );
            cx.notify();
            (revision, expected)
        });
        let deadline = crate::test_wait::hang_deadline();
        let observation = loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if let Ok(observation) =
                editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
            {
                break observation;
            }
            assert!(Instant::now() < deadline, "committed section view");
            std::thread::yield_now();
        };
        let terminal = loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
                break event;
            }
            assert!(Instant::now() < deadline, "section terminal");
            std::thread::yield_now();
        };
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        let committed = observation.state.upgrade().unwrap();
        editor.update(&mut visual, |view, cx| {
            assert!(view.observe_duplicate_view(observation));
            assert_eq!(view.state.as_ref().unwrap().revision, source_revision);
            cx.notify();
        });
        visual.run_until_parked();
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            let ready = editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                view.section_copy_play_source().is_some()
            });
            if ready {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "exact applied source and released fence"
            );
            std::thread::yield_now();
        }
        editor.update(&mut visual, |view, cx| {
            assert_eq!(view.state.as_ref().unwrap().revision, source_revision);
            assert!(Arc::ptr_eq(
                &view.arrangement_rows_state().unwrap(),
                &committed
            ));
            let pending = view.pending_publication_observation.as_mut().unwrap();
            pending.output_identity += 1;
            assert!(
                view.section_copy_play_source().is_none(),
                "wrong output must not start"
            );
            view.pending_publication_observation
                .as_mut()
                .unwrap()
                .output_identity -= 1;
            view.pending_publication_observation
                .as_mut()
                .unwrap()
                .context
                .project_generation += 1;
            assert!(
                view.section_copy_play_source().is_none(),
                "wrong generation must not start"
            );
            view.pending_publication_observation
                .as_mut()
                .unwrap()
                .context
                .project_generation -= 1;
            view.pending_publication_observation
                .as_mut()
                .unwrap()
                .context
                .editor_revision += 1;
            assert!(
                view.section_copy_play_source().is_none(),
                "wrong revision must not start"
            );
            view.pending_publication_observation
                .as_mut()
                .unwrap()
                .context
                .editor_revision -= 1;
            let crate::EditorPublicationState::ResidentSubmitted { metadata, .. } =
                &mut view.pending_publication_observation.as_mut().unwrap().state
            else {
                panic!("resident metadata")
            };
            metadata.requires_preroll = true;
            assert!(
                view.section_copy_play_source().is_none(),
                "preroll must not be bypassed"
            );
            let crate::EditorPublicationState::ResidentSubmitted { metadata, .. } =
                &mut view.pending_publication_observation.as_mut().unwrap().state
            else {
                panic!("resident metadata")
            };
            metadata.requires_preroll = false;
            view.audio_status()
                .unwrap()
                .device_fault
                .store(true, std::sync::atomic::Ordering::Release);
            assert!(
                view.section_copy_play_source().is_none(),
                "faulted output must not start"
            );
            view.audio_status()
                .unwrap()
                .device_fault
                .store(false, std::sync::atomic::Ordering::Release);
            view.toggle_play();
            assert_eq!(
                view.last_transport_command,
                Some(hex_engine::playback::PlaybackCommand::PlayFrom(expected))
            );
            cx.notify();
        });
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        editor.read_with(&visual, |view, _| {
            assert!(view.audio_status().unwrap().playing.load(Acquire))
        });
        editor.update(&mut visual, |view, _| {
            view.pending_publication_observation
                .as_mut()
                .unwrap()
                .output_identity += 1;
            view.toggle_play();
            assert_eq!(
                view.last_transport_command,
                Some(hex_engine::playback::PlaybackCommand::Pause)
            );
            view.pending_publication_observation
                .as_mut()
                .unwrap()
                .output_identity -= 1;
        });
        fixture.backend.render(&mut [[0.0; 2]; 512]).unwrap();
        editor.read_with(&visual, |view, _| {
            assert!(!view.audio_status().unwrap().playing.load(Acquire))
        });
        visual.run_until_parked();
        editor.update(&mut visual, |view, cx| {
            view.receive(terminal);
            assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &committed));
            cx.notify();
        });
        visual.run_until_parked();
        fixture.finish(cx);
    }

    #[gpui::test]
    fn section_keys_during_a_pending_edit_queue_one_follow_up_or_refuse_visibly(
        cx: &mut TestAppContext,
    ) {
        let sections = |editor: &Entity<EditorView>, cx: &mut TestAppContext| {
            editor.read_with(cx, |v, _| {
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_sections()
                    .iter()
                    .map(|s| s.id())
                    .collect::<Vec<_>>()
            })
        };
        // (keys after the pending cmd-d, expected section count, first kept)
        for (keys, count, kept) in [
            // Back to back: duplicate, then delete of the same section.
            ("backspace", 2, false),
            // Latest wins: the delete is replaced by a second duplicate.
            ("backspace cmd-d", 4, true),
            // Undo orders behind the duplicate and drops the queued delete.
            ("backspace cmd-z", 2, true),
        ] {
            let (handle, editor, ids) = fixture(cx);
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            let a = editor.read_with(cx, |v, _| position(v, 2));
            visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            visual.simulate_keystrokes("cmd-d");
            // The acknowledgement is delayed: nothing ticks the worker yet.
            visual.simulate_keystrokes(keys);
            editor.read_with(cx, |v, _| {
                assert!(v.sections.pending.is_some(), "{keys}");
                let queued = v.sections.queued.as_ref().map(|q| &q.command);
                match keys {
                    "backspace" => assert!(
                        matches!(queued, Some(Command::DeleteArrangementSection(id)) if *id == ids[0])
                    ),
                    "backspace cmd-d" => assert!(
                        matches!(queued, Some(Command::DuplicateArrangementSection(id)) if *id == ids[0])
                    ),
                    _ => {
                        assert!(queued.is_none());
                        assert_eq!(&*v.status, "queued section edit cancelled by undo/redo");
                    }
                }
                if queued.is_some() {
                    assert_eq!(
                        &*v.status,
                        "section edit queued · applies after the current edit"
                    );
                }
            });
            settle(&editor, &mut visual);
            let after = sections(&editor, cx);
            assert_eq!(after.len(), count, "{keys}: {after:?}");
            assert_eq!(after.contains(&ids[0]), kept, "{keys}");
            editor.read_with(cx, |v, _| {
                assert!(v.error.is_none(), "{keys}: {:?}", v.error)
            });
        }

        // g queues a merge behind another in-flight section edit.
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let (a, b) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 10)));
        visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(b, MouseButton::Left, shift);
        visual.simulate_mouse_up(b, MouseButton::Left, shift);
        visual.run_until_parked();
        editor.update(cx, |v, _| {
            v.section_metadata(Command::SetArrangementSectionColor {
                section: ids[1],
                color: ElementColor::Green,
            });
            assert!(v.sections.pending.is_some());
        });
        visual.simulate_keystrokes("g");
        editor.read_with(cx, |v, _| {
            assert!(matches!(
                v.sections.queued.as_ref().map(|q| &q.command),
                Some(Command::MergeArrangementSections(_))
            ));
        });
        settle(&editor, &mut visual);
        assert_eq!(sections(&editor, cx), vec![ids[0]]);

        // Pointer and split input cannot wait for the edit; they refuse visibly.
        editor.update(cx, |v, _| {
            v.section_metadata(Command::SetArrangementSectionColor {
                section: ids[0],
                color: ElementColor::Blue,
            });
            assert!(v.sections.pending.is_some());
        });
        visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |v, _| {
            assert_eq!(v.error.as_deref(), Some(SECTION_BUSY));
            assert!(v.sections.gesture.is_none());
        });
        settle(&editor, &mut visual);
    }
    #[gpui::test]
    fn resize_keeps_one_selected_surface_every_frame_through_delayed_ack(cx: &mut TestAppContext) {
        let platform = Modifiers {
            platform: true,
            alt: true,
            ..Modifiers::default()
        };
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        // (touching seam, modifiers, release quarter)
        for (seam, modifiers, end) in [
            (false, Modifiers::default(), 7),
            (false, platform, 7),
            (false, alt, 7),
            (true, Modifiers::default(), 6),
            (true, platform, 6),
        ] {
            let (handle, editor, ids) = if seam {
                touching_fixture(cx)
            } else {
                fixture(cx)
            };
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            let edge = bounds(&mut visual, 0);
            let down = point(
                if seam {
                    edge.right()
                } else {
                    edge.right() - px(1.0)
                },
                edge.origin.y + px(12.0),
            );
            let to = editor.read_with(cx, |v, _| position(v, end));
            visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
            let mut frames = Vec::new();
            // Sub-grid steps cross snap points, so the pointer alternates
            // between inside and outside the snapped preview edge.
            for step in 1..=8 {
                let x = down.x + (to.x - down.x) * (step as f32 / 8.0);
                visual.simulate_mouse_move(point(x, down.y), MouseButton::Left, modifiers);
                visual.run_until_parked();
                assert!(
                    visual
                        .debug_bounds("arrangement-section-selected-trailing-0")
                        .is_some(),
                    "seam {seam} {modifiers:?} step {step}: section lost its selected edge"
                );
                editor.read_with(cx, |v, _| {
                    assert_eq!(v.sections.selected, Some(ids[0]));
                    assert!(v.sections.gesture.is_some());
                });
                frames.push((
                    painted_fill(&mut visual, 0),
                    painted_fill_for(&mut visual, "arrangement-section-header-0"),
                ));
            }
            visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
            let event = editor.update(cx, |v, _| next(&v.service));
            assert!(event.error.is_none(), "{:?}", event.error);
            // Delayed acknowledgement: completed repaints keep the frozen look.
            for _ in 0..3 {
                visual.update(|window, _| window.refresh());
                visual.run_until_parked();
                assert!(
                    visual
                        .debug_bounds("arrangement-section-selected-trailing-0")
                        .is_some()
                );
                frames.push((
                    painted_fill(&mut visual, 0),
                    painted_fill_for(&mut visual, "arrangement-section-header-0"),
                ));
            }
            assert!(frames[0].0.is_some() && frames[0].1.is_some(), "{frames:?}");
            for (index, frame) in frames.iter().enumerate() {
                assert_eq!(
                    frame, &frames[0],
                    "seam {seam} {modifiers:?} frame {index}: selected surface changed"
                );
            }
            editor.update(cx, |v, cx| {
                v.receive(event);
                cx.notify();
            });
            visual.run_until_parked();
            assert!(
                visual
                    .debug_bounds("arrangement-section-selected-trailing-0")
                    .is_some()
            );
            editor.read_with(cx, |v, _| {
                assert_eq!(v.sections.selected, Some(ids[0]));
                assert_eq!(
                    v.state
                        .as_ref()
                        .unwrap()
                        .project
                        .arrangement_section(ids[0])
                        .unwrap()
                        .end(),
                    BeatTime::from_quarters(end).unwrap()
                );
            });
        }
    }
    #[gpui::test]
    fn g_merges_a_click_and_shift_click_section_span(cx: &mut TestAppContext) {
        g_merge_case(cx, false, false);
    }
    #[gpui::test]
    fn g_merges_a_leftward_shift_click_span(cx: &mut TestAppContext) {
        g_merge_case(cx, false, true);
    }
    #[gpui::test]
    fn g_merges_a_leftward_shift_click_span_under_section_focus(cx: &mut TestAppContext) {
        g_merge_case(cx, true, true);
    }
    #[gpui::test]
    fn g_merges_a_shift_click_span_under_section_focus(cx: &mut TestAppContext) {
        g_merge_case(cx, true, false);
    }
    #[gpui::test]
    fn g_merges_a_selected_span_outside_strict_section_focus_and_explains_empty(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        // Nothing selected: g explains instead of silently doing nothing.
        visual.simulate_keystrokes("g");
        editor.read_with(cx, |v, _| {
            assert!(
                v.error
                    .as_ref()
                    .is_some_and(|e| e.contains("select two or more clips, sections")),
                "{:?}",
                v.error
            );
        });
        editor.update(cx, |v, _| {
            v.error = None;
            v.sections.selected = Some(ids[0]);
            v.sections.selection = ids.to_vec();
            // Main no longer owns the device view, so the strict section-focus
            // test fails, yet the section span is the only grouping selection.
            v.device_panel.main_selected = None;
        });
        visual.simulate_keystrokes("g");
        editor.read_with(cx, |v, _| {
            assert!(
                matches!(
                    v.sections.pending.as_ref().map(|p| &p.command),
                    Some(Command::MergeArrangementSections(_))
                ),
                "{:?}",
                v.error
            );
        });
    }
    fn g_merge_case(cx: &mut TestAppContext, focused: bool, leftward: bool) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let (mut a, mut b) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 10)));
        if leftward {
            std::mem::swap(&mut a, &mut b);
        }
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        if focused {
            visual.simulate_keystrokes("q");
            visual.run_until_parked();
            visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
        }
        visual.simulate_mouse_down(b, MouseButton::Left, shift);
        visual.simulate_mouse_up(b, MouseButton::Left, shift);
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            let mut selection = v.sections.selection.clone();
            selection.sort();
            let mut expected = ids.to_vec();
            expected.sort();
            assert_eq!(selection, expected, "error={:?}", v.error);
        });
        visual.simulate_keystrokes("g");
        editor.read_with(cx, |v, _| {
            assert!(
                matches!(
                    v.sections.pending.as_ref().map(|p| &p.command),
                    Some(Command::MergeArrangementSections(_))
                ),
                "error={:?} status={:?} track={:?} clip={:?} time={:?} main={:?}",
                v.error,
                v.status,
                v.selected_track,
                v.selected_clip,
                v.time_selection.map(|t| (t.anchor_frame, t.end_frame)),
                v.device_panel.main_selected,
            );
        });
    }
}
