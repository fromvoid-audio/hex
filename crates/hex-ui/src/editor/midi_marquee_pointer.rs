//! One held input adapter for two immutable hit projections. Live membership,
//! owned rollback and finite hit-entry audition stay in midi_marquee/selection.
use super::{CanvasBounds, EditorView, Marquee, MarqueeContext, Surface};
use gpui::{Context, CursorStyle, MouseButton, MouseMoveEvent, MouseUpEvent, Window};
use hex_project::ClipId;

#[derive(Clone, Copy)]
struct Key {
    context: MarqueeContext,
    first: [f32; 2],
    velocity: bool,
}
impl Surface {
    fn velocity(&self) -> bool {
        matches!(self, Self::Velocity(_))
    }
    fn matches(&self, context: MarqueeContext) -> bool {
        match self {
            Self::Piano(scene) => scene.key() == context.scene,
            Self::Velocity(scene) => {
                let k = scene.key;
                let c = context.scene;
                (k.generation, k.revision, k.clip, k.start, k.span)
                    == (c.generation, c.revision, c.clip, c.beat_start, c.beat_span)
            }
        }
    }
    fn update(
        &self,
        marquee: &mut Marquee,
        context: MarqueeContext,
        selection: &super::super::midi_selection::MidiSelection,
        last: [f32; 2],
    ) -> Result<bool, &'static str> {
        if !self.matches(context) {
            return Err("midi marquee source changed");
        }
        match self {
            Self::Piano(scene) => marquee.update(context, selection, scene, last),
            Self::Velocity(scene) => {
                marquee.update_with_hits(context, selection, last, |first, last| {
                    scene.marquee_hits(first, last, context.bounds.width, context.bounds.height)
                })
            }
        }
    }
}
impl EditorView {
    fn held_marquee(&self, velocity: bool) -> Option<&Marquee> {
        if velocity {
            self.midi_velocity.marquee()
        } else {
            self.midi_marquee.as_ref()
        }
    }
    fn take_held_marquee(&mut self, velocity: bool) -> Option<Marquee> {
        if velocity {
            self.midi_velocity.take_marquee()
        } else {
            self.midi_marquee.take()
        }
    }
    fn install_held_marquee(&mut self, velocity: bool, marquee: Marquee) {
        if velocity {
            self.midi_velocity.install_marquee(marquee);
        } else {
            self.midi_marquee = Some(marquee);
        }
    }
    fn owns_marquee_pointer(&self, key: Key) -> bool {
        self.held_marquee(key.velocity).is_some_and(|m| {
            let mut context = m.context();
            // apply_live advances the model's expected selection epoch. Input
            // admission still belongs to the original scene/bounds/first point.
            context.selection_epoch = key.context.selection_epoch;
            context == key.context && m.rectangle().0 == key.first
        })
    }
    fn cancel_marquee_pointer(&mut self, key: Key) -> bool {
        if !self.owns_marquee_pointer(key) {
            return false;
        }
        if let Some(m) = self.take_held_marquee(key.velocity) {
            self.cancel_live_midi_marquee(m);
            return true;
        }
        false
    }
    fn marquee_pointer_visible(&self, velocity: bool, window: &Window) -> bool {
        self.clip_view_open
            && !self.device_panel_active
            && self.context_menu.is_none()
            && self.midi_focus.is_focused(window)
            && if velocity {
                self.midi_velocity.is_open()
            } else {
                !self.midi_draw_mode
            }
    }
    pub(in crate::editor) fn capture_midi_marquee_pointer(
        &self,
        marquee: &Marquee,
        surface: Surface,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> bool {
        let context = marquee.context();
        if !surface.matches(context) {
            return false;
        }
        let key = Key {
            context,
            first: marquee.rectangle().0,
            velocity: surface.velocity(),
        };
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
                    if !view.owns_marquee_pointer(key) {
                        return;
                    }
                    if !view.marquee_pointer_visible(key.velocity, window)
                        || !f32::from(at.x).is_finite()
                        || !f32::from(at.y).is_finite()
                    {
                        if view.cancel_marquee_pointer(key) {
                            cx.notify();
                        }
                        return;
                    }
                    let context = if key.velocity {
                        view.velocity_marquee_context(key.context.bounds)
                    } else {
                        view.midi_marquee_context(key.context.bounds)
                    };
                    let Some(mut marquee) = view.take_held_marquee(key.velocity) else {
                        return;
                    };
                    let previous = marquee.hits();
                    let last = [
                        f32::from(at.x) - key.context.bounds.x,
                        f32::from(at.y) - key.context.bounds.y,
                    ];
                    let result = context
                        .ok_or("midi marquee source changed")
                        .and_then(|c| surface.update(&mut marquee, c, &view.midi_selection, last))
                        .and_then(|_| view.apply_live_midi_marquee(&mut marquee, &previous));
                    match result {
                        Ok(()) if released.is_none() => {
                            view.install_held_marquee(key.velocity, marquee)
                        }
                        Ok(()) => {} // Release adopts the already visible membership.
                        Err(error) => {
                            view.cancel_live_midi_marquee(marquee);
                            view.error = Some(error.into());
                        }
                    }
                    cx.notify();
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.cancel_marquee_pointer(key) {
                        cx.notify();
                    }
                });
            },
            cx,
        )
    }
    /// Actual rendered identity/scalars and held state only. No project lookup,
    /// hit search, selection mutation or audition work occurs during painting.
    pub(in crate::editor) fn renew_midi_marquee_pointer(
        &self,
        velocity: bool,
        bounds: CanvasBounds,
        clip: ClipId,
        owner: gpui::EntityId,
        window: &mut Window,
    ) {
        if !self.marquee_pointer_visible(velocity, window) || self.detail_clip_id() != Some(clip) {
            return;
        }
        let Some(state) = &self.state else {
            return;
        };
        let context = MarqueeContext {
            scene: super::super::midi_scene::SceneKey {
                generation: self.project_generation,
                revision: state.revision,
                clip,
                beat_start: self.piano_beat_start,
                beat_span: self.piano_beat_span.raw(),
                pitch_low: self.piano_pitch_low,
                pitch_rows: self.piano_pitch_rows,
            },
            bounds,
            selection_epoch: self.midi_selection.epoch(),
        };
        if self
            .held_marquee(velocity)
            .is_some_and(|m| m.is_current(context))
        {
            window.renew_pointer_capture(owner, CursorStyle::Crosshair);
        }
    }
}
