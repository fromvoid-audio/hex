//! Held velocity-value input only. Pure model math and released receipt custody
//! remain separate; raw lane marquee and resize owners are unchanged.
use super::{Context, EditorView};
use gpui::{
    Context as ViewContext, CursorStyle, MouseButton, MouseMoveEvent, MouseUpEvent, Window,
};
use hex_project::Command;

#[derive(Clone, Copy, PartialEq)]
struct Admission {
    context: Context,
    start_y: f32,
}
impl EditorView {
    fn owns_velocity_pointer(&self, key: Admission) -> bool {
        self.midi_velocity
            .gesture
            .as_ref()
            .is_some_and(|g| g.admission() == (key.context, key.start_y))
    }
    fn cancel_velocity_pointer(&mut self, key: Admission) -> bool {
        if !self.owns_velocity_pointer(key) {
            return false;
        }
        self.midi_velocity.gesture = None;
        true
    }
    fn velocity_pointer_visible(&self, window: &Window) -> bool {
        self.clip_view_open
            && !self.device_panel_active
            && self.midi_velocity.open
            && self.context_menu.is_none()
            && self.midi_focus.is_focused(window)
    }
    pub(super) fn capture_velocity_pointer(
        &self,
        context: Context,
        start_y: f32,
        window: &mut Window,
        cx: &ViewContext<Self>,
    ) -> bool {
        let key = Admission { context, start_y };
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            CursorStyle::ResizeUpDown,
            move |event, window, cx| {
                let moved = event.downcast_ref::<MouseMoveEvent>();
                let released = event
                    .downcast_ref::<MouseUpEvent>()
                    .filter(|e| e.button == MouseButton::Left);
                let Some(at) = moved
                    .map(|e| e.position)
                    .or_else(|| released.map(|e| e.position))
                else {
                    return;
                };
                let _ = owner.update(cx, |view, cx| {
                    if !view.owns_velocity_pointer(key) {
                        return;
                    }
                    let current = view.velocity_context(key.context.bounds);
                    if !view.velocity_pointer_visible(window)
                        || view.edit_pending()
                        || current != Some(key.context)
                        || !f32::from(at.x).is_finite()
                        || !f32::from(at.y).is_finite()
                    {
                        if view.cancel_velocity_pointer(key) {
                            cx.notify();
                        }
                        return;
                    }
                    let Some(mut gesture) = view.midi_velocity.gesture.take() else {
                        return;
                    };
                    match gesture.update(key.context, f32::from(at.y)) {
                        Ok(changed) if released.is_none() => {
                            view.midi_velocity.gesture = Some(gesture);
                            if changed {
                                cx.notify();
                            }
                        }
                        Ok(_) => {
                            // Vertical value drags intentionally clamp and commit
                            // outside the lane, matching the existing value model.
                            match gesture.updates(key.context) {
                                Ok(Some(notes)) => view.submit_midi_drag_preview(
                                    key.context.scene.clip,
                                    super::super::midi_pending_drag::Frozen::Velocity(gesture),
                                    Command::UpdateMidiNotes {
                                        clip: key.context.scene.clip,
                                        notes,
                                    },
                                ),
                                Ok(None) => {}
                                Err(error) => view.error = Some(error.into()),
                            }
                            cx.notify();
                        }
                        Err(error) => {
                            view.error = Some(error.into());
                            cx.notify();
                        }
                    }
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.cancel_velocity_pointer(key) {
                        cx.notify();
                    }
                });
            },
            cx,
        )
    }
    /// Paint uses the actual lane's immutable source/scalars and held model.
    /// No project traversal, source allocation, or pending display witness.
    pub(super) fn renew_velocity_pointer(
        &self,
        context: Context,
        owner: gpui::EntityId,
        window: &mut Window,
    ) {
        if self.velocity_pointer_visible(window)
            && self.project_generation == context.scene.generation
            && self
                .state
                .as_ref()
                .is_some_and(|s| s.revision == context.scene.revision)
            && self.piano_beat_start == context.scene.start
            && self.piano_beat_span.raw() == context.scene.span
            && self.midi_selection.epoch() == context.selection_epoch
            && self
                .midi_velocity
                .gesture
                .as_ref()
                .is_some_and(|g| g.is_current(context))
        {
            window.renew_pointer_capture(owner, CursorStyle::ResizeUpDown);
        }
    }
}
