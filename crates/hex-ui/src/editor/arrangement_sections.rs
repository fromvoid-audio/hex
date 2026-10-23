//! Main-lane section display and captured, stamped user intents.
//!
//! Project owns section identity, validation and song-wide permutation. This
//! module projects only marker geometry. Release transfers immutable paint to
//! an ordinary FIFO receipt; Escape/blur cancel held input, never admitted work.
use super::{
    EditorMode, EditorView,
    browser_input::BrowserSearch,
    musical_time::SnapGrid,
    pending_edit::{Disposition, Receipt},
    viewport::Viewport,
};
use crate::{
    context_menu::{MenuItem, PopupMenu},
    theme,
};
use gpui::{
    AnyElement, AppContext, Context, Entity, Subscription, Window, div, prelude::*, px, relative,
    rgb,
};
use hex_project::{ArrangementSection, BeatTime, Command, ElementColor, SectionId, TempoMap};
use hex_session::service::{Event, Request};

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
            id: Some(s.id()),
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
    insertion: Option<u64>,
}
#[derive(Clone, Copy)]
enum Mode {
    Paint,
    Start(SectionId),
    End(SectionId),
    Move(SectionId),
    Reorder(SectionId),
}
struct Gesture {
    generation: u64,
    revision: u64,
    viewport: Viewport,
    snap: SnapGrid,
    layout: [f32; 4],
    anchor: BeatTime,
    anchor_x: f32,
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
    receipt: Receipt,
}
struct Name {
    id: SectionId,
    generation: u64,
    revision: u64,
    input: Entity<BrowserSearch>,
    _blur: Subscription,
}
#[derive(Default)]
pub(super) struct Sections {
    pub(super) selected: Option<SectionId>,
    gesture: Option<Gesture>,
    pending: Option<Pending>,
    name: Option<Name>,
}
impl Sections {
    pub(super) fn held(&self) -> bool {
        self.gesture.is_some()
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
}

impl EditorView {
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
            move |_, _, window, _| {
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
                        if view.gain_numeric_editing(cx)
                            || view.sample_bpm_editing(cx)
                            || view.project_tempo_editing(cx)
                        {
                            return false;
                        }
                        let captured = view.begin_section_pointer(
                            f32::from(event.position.x),
                            f32::from(event.position.y),
                            event.click_count == 2,
                            event.modifiers.alt,
                            event.modifiers.platform,
                            cx,
                        );
                        if captured {
                            view.focus.focus(window);
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
                                event.modifiers.platform,
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
                            event.modifiers.platform,
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
    pub(super) fn section_install_pending(&self) -> bool {
        self.sections.pending.as_ref().is_some_and(|pending| {
            matches!(
                &pending.command,
                Command::ReorderSectionBefore { .. } | Command::ReorderSectionAfter { .. }
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
        self.retain_clip_detail_for_time_selection();
        self.selected_track = None;
        self.selected_clip = None;
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
    fn section_has_focus(&self, window: &Window) -> bool {
        self.mode == EditorMode::Arrangement
            && self.focus.is_focused(window)
            && self.selected_clip.is_none()
            && self.selected_track.is_none()
            && self.focused_return.is_none()
            && self.device_panel.main_selected == Some(self.project_generation)
    }
    fn sections_available(&self) -> bool {
        self.mode == EditorMode::Arrangement
            && !self.edit_pending()
            && self.sections.pending.is_none()
            && !self.section_install_pending()
            && self.sections.name.is_none()
            && self.clip_name_edit.is_none()
            && !self.track_name.is_editing()
            && self.context_menu.is_none()
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
    fn section_current(&self, g: &Gesture) -> bool {
        self.sections_available()
            && g.generation == self.project_generation
            && self.section_geometry() == Some(g.layout)
            && g.snap == self.snap_grid
            && self.state.as_ref().is_some_and(|s| {
                s.revision == g.revision && self.visible_viewport(s.timeline_frames) == g.viewport
            })
    }
    pub(super) fn begin_section_pointer(
        &mut self,
        x: f32,
        y: f32,
        double: bool,
        _alt: bool,
        free: bool,
        cx: &Context<Self>,
    ) -> bool {
        if self.section_pointer(x, y, free).is_none() {
            self.sections.selected = None;
            return false;
        }
        if self.sections.naming()
            || self.gain_numeric_editing(cx)
            || self.sample_bpm_editing(cx)
            || self.project_tempo_editing(cx)
        {
            return false;
        }
        let Some(anchor) = self.section_pointer(x, y, free) else {
            self.sections.selected = None;
            return false;
        };
        if !self.sections_available() {
            return self.section_hit(x, y).is_some();
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let id = self.section_hit(x, y);
        if id.is_none() && !double {
            self.sections.selected = None;
            return false;
        }
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(layout) = self.section_layout() else {
            return false;
        };
        let mut original = id
            .and_then(|id| state.project.arrangement_section(id))
            .map(Marker::from);
        let mode = if let (Some(marker), Some(id)) = (&original, id) {
            let position = |b| {
                state.project.tempo_map().beat_to_frame(b).ok().map(|f| {
                    layout[0]
                        + (f as f64 - viewport.start as f64) as f32 / viewport.length as f32
                            * layout[2]
                })
            };
            if position(marker.start).is_some_and(|p| (p - x).abs() <= 5.0) {
                Mode::Start(id)
            } else if position(marker.end).is_some_and(|p| (p - x).abs() <= 5.0) {
                Mode::End(id)
            } else if free {
                Mode::Move(id)
            } else {
                if self
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.control.status())
                    .is_some_and(|status| status.playing.load(std::sync::atomic::Ordering::Acquire))
                {
                    self.select_section_metadata(Some(id));
                    self.error = Some("stop playback before reordering sections".into());
                    return true;
                }
                Mode::Reorder(id)
            }
        } else {
            Mode::Paint
        };
        if matches!(mode, Mode::Paint) {
            let (name, color) = new_marker_appearance(&state.project);
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
            insertion: None,
        };
        self.sections.gesture = Some(Gesture {
            generation: self.project_generation,
            revision: state.revision,
            viewport,
            snap: self.snap_grid,
            layout,
            anchor,
            anchor_x: x,
            original,
            mode,
            display,
            command: None,
        });
        self.select_section_metadata(id);
        self.status = if matches!(mode, Mode::Reorder(_)) {
            "section drag · insert before or after a section to move song content"
        } else {
            "section marker edit · musical content stays in place"
        }
        .into();
        self.update_section_pointer(x, y, free);
        true
    }
    pub(super) fn update_section_pointer(&mut self, x: f32, y: f32, free: bool) -> bool {
        let Some(mut g) = self.sections.gesture.take() else {
            return false;
        };
        if !self.section_current(&g) || self.section_layout().is_none() {
            return true;
        }
        g.command = None;
        g.display.insertion = None;
        let Some(beat) = self.section_pointer(x, y, free) else {
            self.sections.gesture = Some(g);
            return true;
        };
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
            insertion: None,
        };
        match g.mode {
            Mode::Reorder(id) => {
                // Structural destinations are fixed edges, independent of grid snap.
                // A click must never move music, even when it snaps outside its marker.
                let raw_beat = self.section_pointer(x, y, true).unwrap_or(beat);
                if (x - g.anchor_x).abs() >= 4.0
                    && let Some((target, after)) = insertion_target(&state.project, id, raw_beat)
                {
                    let preview = if after {
                        state.project.preview_section_reorder_after(id, target)
                    } else {
                        state.project.preview_section_reorder(id, target)
                    };
                    match preview {
                        Ok(preview) if !preview.is_noop() => {
                            g.display.markers =
                                preview.sections().iter().map(Marker::from).collect();
                            g.display.tempo = preview.tempo_map().clone();
                            g.display.insertion = state
                                .project
                                .tempo_map()
                                .beat_to_frame(preview.insertion_beat())
                                .ok();
                            g.command = Some(if after {
                                Command::ReorderSectionAfter {
                                    section: id,
                                    after: target,
                                }
                            } else {
                                Command::ReorderSectionBefore {
                                    section: id,
                                    before: target,
                                }
                            });
                        }
                        Ok(_) => {}
                        Err(error) => self.error = Some(error.to_string().into()),
                    }
                }
            }
            mode => {
                let (id, start, end) = match (mode, g.original.as_ref()) {
                    (Mode::Paint, _) => {
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
                        (
                            None,
                            beat.min(g.anchor),
                            if beat == g.anchor {
                                minimum
                            } else {
                                beat.max(g.anchor)
                            },
                        )
                    }
                    (Mode::Start(id), Some(m)) => (Some(id), beat, m.end),
                    (Mode::End(id), Some(m)) => (Some(id), m.start, beat),
                    (Mode::Move(id), Some(m)) => {
                        let delta = i128::from(beat.raw()) - i128::from(g.anchor.raw());
                        let (Ok(start), Ok(end)) = (
                            u64::try_from(i128::from(m.start.raw()) + delta),
                            u64::try_from(i128::from(m.end.raw()) + delta),
                        ) else {
                            self.sections.gesture = Some(g);
                            return true;
                        };
                        (Some(id), BeatTime::from_raw(start), BeatTime::from_raw(end))
                    }
                    _ => {
                        self.sections.gesture = Some(g);
                        return true;
                    }
                };
                let valid = start < end
                    && state
                        .project
                        .arrangement_sections()
                        .iter()
                        .filter(|s| Some(s.id()) != id)
                        .all(|s| end <= s.start() || start >= s.end());
                if valid {
                    if let Some(id) = id {
                        if let Some(marker) =
                            g.display.markers.iter_mut().find(|m| m.id == Some(id))
                        {
                            marker.start = start;
                            marker.end = end;
                        }
                        if g.original
                            .as_ref()
                            .is_some_and(|m| m.start != start || m.end != end)
                        {
                            g.command = Some(Command::SetArrangementSectionRange {
                                section: id,
                                start,
                                end,
                            });
                        }
                    } else {
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
        }
        self.sections.gesture = Some(g);
        true
    }
    pub(super) fn finish_section_pointer(&mut self, x: f32, y: f32, free: bool) -> bool {
        if self.sections.gesture.is_none() {
            return false;
        }
        self.update_section_pointer(x, y, free);
        let Some(g) = self.sections.gesture.take() else {
            return true;
        };
        self.suppress_cursor_click = true;
        if self.section_current(&g)
            && self.section_pointer(x, y, free).is_some()
            && let Some(command) = g.command
        {
            self.submit_section(command, g.display, g.generation, g.revision);
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
        self.sections.pending = Some(Pending {
            generation,
            revision,
            command: command.clone(),
            display,
            receipt: Receipt::new(generation),
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
        let Some(pending) = self.sections.pending.as_ref() else {
            return false;
        };
        let Request::EditArrangementSection {
            project_generation,
            revision,
            command,
        } = request
        else {
            return false;
        };
        pending.receipt.is_queued()
            && *project_generation == pending.generation
            && *revision == pending.revision
            && same_command(command, &pending.command)
    }
    pub(super) fn section_submitted(&mut self) {
        if let Some(p) = self.sections.pending.as_mut() {
            p.receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(super) fn retire_pending_section(&mut self) -> bool {
        self.sections.pending.take().is_some()
    }
    pub(super) fn section_request_complete(&mut self, event: &Event) {
        if self.sections.pending.as_mut().is_some_and(|p| {
            p.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            ) != Disposition::Waiting
        }) {
            self.retire_pending_section();
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
    pub(super) fn reconcile_sections(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .sections
            .gesture
            .as_ref()
            .is_some_and(|g| !self.section_current(g) || self.section_layout().is_none())
        {
            self.cancel_section_pointer();
        }
        if self.sections.selected.is_some()
            && (self.mode != EditorMode::Arrangement
                || self.selected_clip.is_some()
                || self.selected_track.is_some()
                || self.focused_return.is_some()
                || self.device_panel.main_selected != Some(self.project_generation)
                || (!self.focus.is_focused(window)
                    && !self.sections.naming()
                    && self.context_menu.is_none()))
        {
            self.sections.selected = None;
        }
        if self.sections.selected.is_some_and(|id| {
            self.state
                .as_ref()
                .is_none_or(|s| s.project.arrangement_section(id).is_none())
        }) {
            self.sections.selected = None;
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
        let input = cx.new(|cx| BrowserSearch::for_clip_name(section.name().into(), cx));
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
        let name = n.input.read(cx).content.clone();
        if self.section_stamp_current(n.generation, n.revision, n.id) && !self.edit_pending() {
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
        let draft = &name.input.read(cx).content;
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
        let Some(id) = self.sections.selected else {
            return false;
        };
        if !self.section_has_focus(window) {
            return false;
        }
        if self.sections_available() {
            self.cancel_section_pointer();
            self.section_metadata(Command::RemoveArrangementSection(id));
        }
        true
    }
    fn section_metadata(&mut self, command: Command) {
        if !self.sections_available() {
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
            insertion: None,
        };
        match &command {
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
            _ => return,
        }
        self.submit_section(command, display, self.project_generation, state.revision);
    }
    pub(super) fn section_menu(
        &mut self,
        id: SectionId,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sections_available() {
            return;
        }
        self.cancel_section_pointer();
        self.select_section_metadata(Some(id));
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
                label: "rename section · cmd-r",
                swatch: None,
                checked: false,
                action: make(MenuKind::Rename),
            },
            MenuItem {
                label: "remove section marker",
                swatch: None,
                checked: false,
                action: make(MenuKind::Remove),
            },
        ];
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
            MenuKind::Remove => self.section_metadata(Command::RemoveArrangementSection(a.section)),
        }
        cx.notify();
    }
    pub(super) fn section_elements(
        &self,
        viewport: Viewport,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(state) = self.state.as_ref() else {
            return Vec::new();
        };
        let canonical;
        let display = if let Some(p) = self.sections.pending.as_ref() {
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
                insertion: None,
            };
            &canonical
        };
        let Some(limit) = viewport.end() else {
            return Vec::new();
        };
        if viewport.length == 0 {
            return Vec::new();
        }
        let mut elements = Vec::new();
        for (index, m) in display.markers.iter().enumerate() {
            let (Ok(start), Ok(end)) = (
                display.tempo.beat_to_frame(m.start),
                display.tempo.beat_to_frame(m.end),
            ) else {
                continue;
            };
            let (start, end) = (start.max(viewport.start), end.min(limit));
            if end <= start {
                continue;
            }
            let selected = m.id.is_some() && m.id == self.sections.selected;
            let border = if selected {
                theme::color(theme::PaletteRole::Text)
            } else {
                theme::element_colors(m.color).accent
            };
            let mut element = div()
                .id(("arrangement-section", index))
                .debug_selector(move || format!("arrangement-section-{index}"))
                .absolute()
                .top(px(3.0))
                .bottom(px(3.0))
                .left(relative(
                    (start - viewport.start) as f32 / viewport.length as f32,
                ))
                .w(relative((end - start) as f32 / viewport.length as f32))
                .cursor(gpui::CursorStyle::OpenHand)
                .overflow_hidden()
                .border_1()
                .border_color(rgb(border))
                .bg(rgb(theme::clip_fill(
                    m.color,
                    hex_project::ClipFlavor::Accent,
                    false,
                )))
                .text_xs()
                .px_1()
                .tooltip(|_, cx| {
                    cx.new(|_| super::waveform::ClipTooltip {
                        name: "drag: reorder song · command-drag: marker only · drag edges: resize"
                            .into(),
                    })
                    .into()
                });
            if let Some(id) = m.id {
                element = element.on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                        view.section_menu(id, event.position, window, cx);
                        cx.stop_propagation();
                    }),
                );
                if let Some(n) = self.sections.name.as_ref().filter(|n| n.id == id) {
                    element = element.children([n.input.clone()]);
                } else {
                    element = element.child(theme::text(&m.name));
                }
            } else {
                element = element.child(theme::text(&m.name));
            }
            for right in [false, true] {
                let edge = div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .w(px(4.0))
                    .cursor(gpui::CursorStyle::ResizeLeftRight);
                element = element.children([if right { edge.right_0() } else { edge.left_0() }]);
            }
            elements.push(element.into_any_element());
        }
        if let Some(frame) = display
            .insertion
            .filter(|f| *f >= viewport.start && *f <= limit)
        {
            elements.push(
                div()
                    .debug_selector(|| "section-insertion-preview".into())
                    .absolute()
                    .top_0()
                    .left(relative(
                        (frame - viewport.start) as f32 / viewport.length as f32,
                    ))
                    .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                    .child(theme::text("▾"))
                    .into_any_element(),
            );
        }
        elements
    }
}
fn insertion_target(
    project: &hex_project::Project,
    source: SectionId,
    beat: BeatTime,
) -> Option<(SectionId, bool)> {
    if project
        .arrangement_section(source)
        .is_some_and(|s| s.start() <= beat && beat < s.end())
    {
        return None;
    }
    project
        .arrangement_sections()
        .iter()
        .filter(|s| s.id() != source)
        .flat_map(|s| [(s.id(), false, s.start()), (s.id(), true, s.end())])
        .min_by_key(|(_, _, at)| at.raw().abs_diff(beat.raw()))
        .map(|(id, after, _)| (id, after))
}
fn new_marker_appearance(project: &hex_project::Project) -> (String, ElementColor) {
    use std::hash::BuildHasher;
    // Pick once at capture; preview/release and saved history retain that choice.
    let ticket = std::collections::hash_map::RandomState::new()
        .hash_one(project.arrangement_sections().len());
    let color =
        ElementColor::TRACK_CHOICES[(ticket % ElementColor::TRACK_CHOICES.len() as u64) as usize];
    let number = (1..=hex_project::MAX_ARRANGEMENT_SECTIONS + 1)
        .find(|n| {
            let name = format!("section {n}");
            !project
                .arrangement_sections()
                .iter()
                .any(|s| s.name() == name)
        })
        .unwrap_or(hex_project::MAX_ARRANGEMENT_SECTIONS + 1);
    (format!("section {number}"), color)
}
fn same_command(a: &Command, b: &Command) -> bool {
    match (a, b) {
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
        (Command::RemoveArrangementSection(a), Command::RemoveArrangementSection(b)) => a == b,
        _ => false,
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{
        Modifiers, MouseButton, MouseDownEvent, TestAppContext, VisualTestContext, point, px,
    };
    use hex_project::Created;
    use hex_session::service::Service;
    use std::time::{Duration, Instant};

    #[track_caller]
    fn next(service: &Service) -> Event {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(event) = service.try_event() {
                return event;
            }
            assert!(Instant::now() < deadline, "service completion");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn fixture(
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
    fn position(view: &EditorView, quarters: u64) -> gpui::Point<gpui::Pixels> {
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
            px(top + 18.0),
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
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        let target = editor.read_with(cx, |v, _| position(v, 6));
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(matches!(
                v.sections.gesture.as_ref().unwrap().command,
                Some(Command::SetArrangementSectionRange { section, end, .. })
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
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(anchor, MouseButton::Left, alt);
        visual.simulate_mouse_move(destination, MouseButton::Left, alt);
        visual.run_until_parked();
        editor.read_with(cx,|v,_|{
            assert!(matches!(
                v.sections.gesture.as_ref().unwrap().command,
                Some(Command::SetArrangementSectionRange { section, start, end })
                    if section == ids[0] && start == BeatTime::ONE && end == BeatTime::from_quarters(5).unwrap()
            ));
            assert_eq!(v.state.as_ref().unwrap().project,original);
        });
        assert!(visual.debug_bounds("section-insertion-preview").is_none());
        visual.simulate_keystrokes("escape");
    }
    #[test]
    fn section_insertion_uses_edges_and_does_not_move_a_clicked_source() {
        let mut e = hex_project::Editor::new(hex_project::Project::new(48000).unwrap(), 8).unwrap();
        let mut ids = Vec::new();
        for (start, end) in [(0, 4), (8, 12)] {
            let Created::ArrangementSection(id) = e
                .execute(Command::AddArrangementSection {
                    name: format!("section {}", ids.len() + 1),
                    color: ElementColor::Cyan,
                    start: BeatTime::from_quarters(start).unwrap(),
                    end: BeatTime::from_quarters(end).unwrap(),
                })
                .unwrap()
            else {
                panic!("section")
            };
            ids.push(id);
        }
        let p = e.project();
        assert_eq!(
            insertion_target(p, ids[0], BeatTime::from_quarters(2).unwrap()),
            None
        );
        assert_eq!(
            insertion_target(p, ids[0], BeatTime::from_quarters(9).unwrap()),
            Some((ids[1], false))
        );
        assert_eq!(
            insertion_target(p, ids[0], BeatTime::from_quarters(13).unwrap()),
            Some((ids[1], true))
        );
        let (name, color) = new_marker_appearance(p);
        assert_eq!(name, "section 3");
        assert!(ElementColor::TRACK_CHOICES.contains(&color));
    }
    #[gpui::test]
    fn first_section_can_be_inserted_after_last(cx: &mut TestAppContext) {
        let (handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let (down, to) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 13)));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(
                v.sections.gesture.as_ref().unwrap().command.is_some(),
                "after-last drop must move the song"
            );
        });
        assert!(visual.debug_bounds("section-insertion-preview").is_some());
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
    fn command_move_changes_canonical_marker_without_moving_music(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let tracks = editor.read_with(cx, |v, _| {
            v.state.as_ref().unwrap().project.tracks().to_vec()
        });
        let (down, to) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 3)));
        let modifiers = Modifiers {
            platform: true,
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
        let (down, to) = editor.read_with(cx, |v, _| (position(v, 10), position(v, 2)));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("section-insertion-preview").is_some());
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
                generation: v.project_generation,
                revision: g.revision,
                command,
                display: g.display,
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
            assert!(visual.debug_bounds("section-insertion-preview").is_some());
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
                    .content
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
                v.new_project(hex_session::DocumentAudioMode::Resident);
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
            assert_eq!(input.content, "Bridge X");
            assert_eq!(input.painted_text_for_test(), Some("bridge x"));
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
        let original_fill = visual.rendered_background("arrangement-section-0");
        visual.simulate_mouse_down(down, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("down down down down down down enter");
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
        assert_ne!(
            visual.rendered_background("arrangement-section-0"),
            original_fill
        );
        let tracks = editor.read_with(cx, |v, _| {
            v.state.as_ref().unwrap().project.tracks().to_vec()
        });
        visual.simulate_keystrokes("backspace");
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
        let (source, target) = editor.read_with(cx, |v, _| (position(v, 10), position(v, 2)));
        visual.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("section-insertion-preview").is_some());
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
}
