//! Held draw input only; the pure stroke owns resolution and the ordinary
//! pending receipt owns released display. Immutable admission keys never paint.
use super::{EditorView, StrokeContext, StrokeModifiers, same_rendered_frame};
use gpui::{Context, CursorStyle, MouseButton, MouseMoveEvent, MouseUpEvent, Window};

type Admission = (StrokeContext, u64, u8);
fn rectangle(c: StrokeContext) -> [f32; 4] {
    [c.bounds.x, c.bounds.y, c.bounds.width, c.bounds.height]
}
impl EditorView {
    fn owns_draw_pointer(&self, key: Admission) -> bool {
        self.midi_stroke
            .as_ref()
            .is_some_and(|s| s.admission() == key)
    }
    fn cancel_draw_pointer(&mut self, key: Admission) -> bool {
        self.owns_draw_pointer(key) && self.cancel_midi_stroke()
    }
    fn draw_pointer_visible(&self, window: &Window) -> bool {
        self.clip_view_open
            && !self.device_panel_active
            && self.midi_draw_mode
            && self.context_menu.is_none()
            && self.midi_focus.is_focused(window)
    }
    pub(super) fn capture_midi_draw_pointer(
        &self,
        stroke: &super::midi_stroke::MidiStroke,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> bool {
        let key = stroke.admission();
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            CursorStyle::Crosshair,
            move |input, window, cx| {
                let moved = input.downcast_ref::<MouseMoveEvent>();
                let released = input
                    .downcast_ref::<MouseUpEvent>()
                    .filter(|e| e.button == MouseButton::Left);
                let Some(at) = moved
                    .map(|e| e.position)
                    .or_else(|| released.map(|e| e.position))
                else {
                    return;
                };
                let _ = owner.update(cx, |view, cx| {
                    if !view.owns_draw_pointer(key) {
                        return;
                    }
                    if !view.draw_pointer_visible(window)
                        || !f32::from(at.x).is_finite()
                        || !f32::from(at.y).is_finite()
                    {
                        if view.cancel_draw_pointer(key) {
                            cx.notify();
                        }
                        return;
                    }
                    let modifiers = moved
                        .map(|e| e.modifiers)
                        .or_else(|| released.map(|e| e.modifiers))
                        .unwrap_or_default();
                    let point = [f32::from(at.x), f32::from(at.y)];
                    let bounds = rectangle(key.0);
                    let modifiers = StrokeModifiers::from_platform(modifiers);
                    if released.is_some() {
                        view.finish_midi_draw(
                            bounds,
                            point,
                            super::midi_drag::point_is_inside(bounds, point[0], point[1]),
                            modifiers,
                        );
                        cx.notify();
                    } else if view.update_midi_draw(bounds, point, modifiers) {
                        cx.notify();
                    }
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.cancel_draw_pointer(key) {
                        cx.notify();
                    }
                });
            },
            cx,
        )
    }
    /// Pair actual rendered bounds/source with current scalar stamps. The
    /// captured minimum gate is pinned to this generation/revision; paint never
    /// prepares timing, scans project notes or renews accepted pending display.
    pub(in crate::editor) fn renew_midi_draw_pointer(
        &self,
        rendered: StrokeContext,
        owner: gpui::EntityId,
        window: &mut Window,
    ) {
        let Some(stroke) = &self.midi_stroke else {
            return;
        };
        let captured = stroke.admission().0;
        if self.draw_pointer_visible(window)
            && self.project_generation == captured.generation
            && self
                .state
                .as_ref()
                .is_some_and(|s| s.revision == captured.revision)
            && self.detail_clip_id() == Some(captured.clip)
            && self.piano_beat_start == captured.view_start
            && self.piano_beat_span.raw() == captured.view_span
            && self.piano_pitch_low == captured.pitch_low
            && self.piano_pitch_rows == captured.pitch_rows
            && self.midi_snap_grid == captured.grid
            && captured.pitch_locked == (self.midi_draw_pitch_locked ^ captured.pitch_modifier)
            && stroke.modifiers_match(window.modifiers().platform, window.modifiers().alt)
            && same_rendered_frame(captured, rendered)
        {
            window.renew_pointer_capture(owner, CursorStyle::Crosshair);
        }
    }
}
