//! Source-local time selection shares ruler geometry and the existing edit-reply
//! selection owner. Drag selection does not seek or reflect Arrangement state;
//! stationary ruler clicks delegate playback cueing to the existing cue owner,
//! while stationary piano/velocity clicks only move the insertion cursor.
//! The Window lease owns held mouse input only; committed selection and canonical
//! edits retain their existing epoch and ordinary worker-receipt owners.
use super::*;
use hex_project::BeatTime;

#[derive(Clone, Copy)]
struct Selection {
    generation: u64,
    clip: ClipId,
    start: u64,
    end: u64,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Surface {
    Ruler,
    Piano,
    Velocity,
}
#[derive(Clone, Copy)]
pub(in crate::editor) struct Capture {
    context: Context,
    note_epoch: u64,
    down_x: f32,
    last_x: f32,
    invert: bool,
    surface: Surface,
}
#[derive(Default)]
pub(in crate::editor) struct State {
    selection: Option<Selection>,
    pub(in crate::editor) gesture: Option<Capture>,
    epoch: u64,
}
impl State {
    pub(in crate::editor) fn cancel(&mut self) -> bool {
        self.gesture.take().is_some()
    }
    fn set(&mut self, selection: Selection) -> Result<(), &'static str> {
        let next = self
            .epoch
            .checked_add(1)
            .ok_or("time selection identity exhausted")?;
        self.selection = Some(selection);
        self.epoch = next;
        Ok(())
    }
    fn clear(&mut self) -> bool {
        let changed = self.selection.take().is_some();
        if changed {
            self.epoch = self.epoch.saturating_add(1);
        }
        changed
    }
}
fn beat_at(context: Context, x: f32, invert: bool) -> Option<u64> {
    let raw = midi_drag::beat_at_point(
        x,
        [
            context.bounds.x,
            context.bounds.y,
            context.bounds.width,
            context.bounds.height,
        ],
        context.start,
        context.span,
    )?;
    let grid = midi_drag::effective_grid(context.grid, invert);
    Some(clamp_loop_grid(
        midi_drag::snap_raw(raw, grid),
        0,
        super::super::midi_scene::zoom_limit(context.values.source_length),
        grid,
    ))
}
impl Capture {
    fn interval(self) -> Option<(u64, u64)> {
        if (self.last_x - self.down_x).abs() < 3.0 {
            return None;
        }
        let first = beat_at(self.context, self.down_x, self.invert)?;
        let last = beat_at(self.context, self.last_x, self.invert)?;
        (first != last).then_some((first.min(last), first.max(last)))
    }
}
impl EditorView {
    pub(in crate::editor) fn shift_midi_time_selection_origin(&mut self, delta: i128) {
        self.midi_time_selection.gesture = None;
        if let Some(selection) = self.midi_time_selection.selection.as_mut() {
            let shift = |raw: u64| u64::try_from(i128::from(raw).saturating_add(delta)).ok();
            if let Some((start, end)) = shift(selection.start).zip(shift(selection.end)) {
                selection.start = start;
                selection.end = end;
            } else {
                self.midi_time_selection.clear();
            }
        }
    }

    pub(in crate::editor) fn midi_marker_at(
        &self,
        bounds: CanvasBounds,
        x: f32,
        invert: bool,
    ) -> Option<u64> {
        beat_at(self.midi_ruler_context(bounds)?, x, invert)
    }

    pub(in crate::editor) fn clear_midi_time_selection(&mut self) -> bool {
        self.midi_time_selection.clear()
    }
    pub(in crate::editor) fn midi_time_selection_epoch_matches(&self, epoch: u64) -> bool {
        self.midi_time_selection.epoch == epoch
            && self.midi_time_selection.selection.is_some_and(|s| {
                s.generation == self.project_generation && self.detail_clip_id() == Some(s.clip)
            })
    }
    pub(in crate::editor) fn midi_time_copy_epoch(
        &self,
        clip: ClipId,
        start: u64,
        end: u64,
    ) -> Option<u64> {
        let s = self.midi_time_selection.selection?;
        (s.generation == self.project_generation
            && s.clip == clip
            && s.start == start
            && s.end == end)
            .then_some(self.midi_time_selection.epoch)
    }
    pub(in crate::editor) fn apply_midi_time_copy(&mut self, clip: ClipId, start: u64, end: u64) {
        if self.detail_clip_id() != Some(clip) {
            return;
        }
        if let Err(error) = self.midi_time_selection.set(Selection {
            generation: self.project_generation,
            clip,
            start,
            end,
        }) {
            self.error = Some(error.into());
            return;
        }
        self.midi_selection.clear();
        self.midi_marker = Some(start);
        // A successful duplicate must remain visible even after it grows beyond
        // the old viewport. Move only as far as needed; this is view state.
        let span = self.piano_beat_span.raw().max(end - start);
        let old = self.piano_beat_start;
        let reveal = if i128::from(start) < old {
            i128::from(start)
        } else if i128::from(end) > old.saturating_add(i128::from(span)) {
            i128::from(end.saturating_sub(span))
        } else {
            old
        };
        if self.midi_clip().is_some() {
            self.piano_beat_span = BeatTime::from_raw(span);
            self.piano_beat_start = super::super::midi_scene::clamp_start(reveal, span);
        }
    }
    pub(in crate::editor) fn midi_time_selection_bounds(&self) -> Option<(u64, u64)> {
        if let Some(capture) = self.midi_time_selection.gesture {
            return capture.interval();
        }
        let s = self.midi_time_selection.selection?;
        (s.generation == self.project_generation && self.detail_clip_id() == Some(s.clip))
            .then_some((s.start, s.end))
    }
    pub(in crate::editor) fn duplicate_midi_time_selection(&mut self) -> bool {
        let Some(s) = self.midi_time_selection.selection.filter(|s| {
            s.generation == self.project_generation && self.detail_clip_id() == Some(s.clip)
        }) else {
            return false;
        };
        if self.edit_pending() {
            return true;
        }
        self.cancel_midi_note_gesture();
        self.enqueue(Request::Edit(
            hex_project::Command::DuplicateMidiSourceRange {
                clip: s.clip,
                start: BeatTime::from_raw(s.start),
                end: BeatTime::from_raw(s.end),
            },
        ));
        true
    }
    pub(super) fn begin_midi_time_selection(
        &mut self,
        context: Context,
        x: f32,
        invert: bool,
        window: &mut gpui::Window,
        cx: &mut ViewContext<Self>,
    ) -> bool {
        self.begin_midi_time_capture(context, x, invert, Surface::Ruler, window, cx)
    }
    /// A stationary grid click moves only the insertion cursor, never playback.
    pub(in crate::editor) fn begin_midi_grid_time_selection(
        &mut self,
        bounds: CanvasBounds,
        x: f32,
        invert: bool,
        window: &mut gpui::Window,
        cx: &mut ViewContext<Self>,
    ) -> bool {
        let Some(context) = self.midi_ruler_context(bounds) else {
            return false;
        };
        self.begin_midi_time_capture(context, x, invert, Surface::Piano, window, cx)
    }
    pub(in crate::editor) fn begin_midi_velocity_time_selection(
        &mut self,
        bounds: CanvasBounds,
        x: f32,
        invert: bool,
        window: &mut gpui::Window,
        cx: &mut ViewContext<Self>,
    ) -> bool {
        let Some(context) = self.midi_ruler_context(bounds) else {
            return false;
        };
        self.begin_midi_time_capture(context, x, invert, Surface::Velocity, window, cx)
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(in crate::editor) fn midi_velocity_time_selection_active(&self) -> bool {
        self.midi_time_selection
            .gesture
            .is_some_and(|g| g.surface == Surface::Velocity)
    }
    fn owns_midi_time_capture(&self, context: Context, surface: Surface, down_x: f32) -> bool {
        self.midi_time_selection
            .gesture
            .is_some_and(|g| g.context == context && g.surface == surface && g.down_x == down_x)
    }
    fn begin_midi_time_capture(
        &mut self,
        context: Context,
        x: f32,
        invert: bool,
        surface: Surface,
        window: &mut gpui::Window,
        cx: &mut ViewContext<Self>,
    ) -> bool {
        if !x.is_finite()
            || self.midi_ruler_context(context.bounds) != Some(context)
            || beat_at(context, x, invert).is_none()
        {
            return false;
        }
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        if !window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            self.time_selection_pointer_cursor(gpui::CursorStyle::Crosshair),
            move |event, window, cx| {
                let moved = event.downcast_ref::<MouseMoveEvent>();
                let released = event
                    .downcast_ref::<MouseUpEvent>()
                    .filter(|e| e.button == MouseButton::Left);
                let Some(position) = moved
                    .map(|e| e.position)
                    .or_else(|| released.map(|e| e.position))
                else {
                    return;
                };
                let _ = owner.update(cx, |view, cx| {
                    if !view.owns_midi_time_capture(context, surface, x) {
                        return;
                    }
                    if !view.clip_view_open
                        || view.device_panel_active
                        || !view.midi_focus.is_focused(window)
                        || !f32::from(position.x).is_finite()
                        || !f32::from(position.y).is_finite()
                    {
                        view.midi_time_selection.cancel();
                    } else {
                        let invert = moved
                            .map(|e| e.modifiers.platform)
                            .or_else(|| released.map(|e| e.modifiers.platform))
                            .unwrap_or(false);
                        if released.is_some() {
                            view.finish_midi_time_selection(
                                context.bounds,
                                f32::from(position.x),
                                invert,
                            );
                        } else {
                            view.update_midi_time_selection(
                                context.bounds,
                                f32::from(position.x),
                                invert,
                            );
                        }
                    }
                    cx.notify();
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.owns_midi_time_capture(context, surface, x) {
                        view.midi_time_selection.cancel();
                        cx.notify();
                    }
                });
            },
            cx,
        ) {
            return false;
        }
        self.cancel_midi_note_gesture();
        self.midi_hover = None;
        self.midi_time_selection.gesture = Some(Capture {
            context,
            note_epoch: self.midi_selection.epoch(),
            down_x: x,
            last_x: x,
            invert,
            surface,
        });
        true
    }
    /// Each surface witnesses only its actual bounds. Pending edits and stored
    /// selection never renew another gesture's lease on the shared Editor entity.
    pub(in crate::editor) fn renew_midi_time_selection_capture(
        &self,
        bounds: CanvasBounds,
        clip: ClipId,
        owner: gpui::EntityId,
        window: &mut gpui::Window,
    ) {
        if self.clip_view_open
            && !self.device_panel_active
            && self.midi_focus.is_focused(window)
            && let Some(g) = self.midi_time_selection.gesture
            && g.context.bounds == bounds
            && g.context.generation == self.project_generation
            && clip == g.context.clip
            && !self.edit_pending()
            && self.context_menu.is_none()
            && self.piano_beat_start == g.context.start
            && self.piano_beat_span.raw() == g.context.span
            && self.midi_snap_grid == g.context.grid
            && self.midi_selection.epoch() == g.note_epoch
            && self
                .state
                .as_ref()
                .is_some_and(|s| s.revision == g.context.revision)
        {
            window.renew_pointer_capture(
                owner,
                self.time_selection_pointer_cursor(gpui::CursorStyle::Crosshair),
            );
        }
    }
    pub(in crate::editor) fn update_midi_time_selection(
        &mut self,
        bounds: CanvasBounds,
        x: f32,
        invert: bool,
    ) -> bool {
        let Some(capture) = self.midi_time_selection.gesture else {
            return false;
        };
        if self.midi_ruler_context(bounds) != Some(capture.context)
            || self.midi_selection.epoch() != capture.note_epoch
            || !x.is_finite()
        {
            return self.midi_time_selection.cancel();
        }
        let mut next = capture;
        next.last_x = x;
        next.invert = invert;
        self.midi_time_selection.gesture = Some(next);
        true
    }
    pub(in crate::editor) fn finish_midi_time_selection(
        &mut self,
        bounds: CanvasBounds,
        x: f32,
        invert: bool,
    ) {
        self.update_midi_time_selection(bounds, x, invert);
        let Some(capture) = self.midi_time_selection.gesture.take() else {
            return;
        };
        let Some((start, end)) = capture.interval() else {
            // A stationary blank-ruler click cues playback. Source-start marker
            // drags and real time selections retain their separate owners.
            if (capture.last_x - capture.down_x).abs() < 3.0
                && let Some(beat) = beat_at(capture.context, capture.last_x, capture.invert)
            {
                if capture.surface == Surface::Ruler {
                    self.request_midi_source_cue(capture.context.clip, beat);
                } else {
                    self.clear_midi_time_selection();
                    self.midi_selection.clear();
                    self.midi_marker = Some(beat);
                }
            }
            return;
        };
        if let Err(error) = self.midi_time_selection.set(Selection {
            generation: self.project_generation,
            clip: capture.context.clip,
            start,
            end,
        }) {
            self.error = Some(error.into());
            return;
        }
        self.midi_selection.clear();
        self.midi_marker = Some(start);
    }
}
