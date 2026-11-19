//! View-only key-gutter zoom through the existing shared splitter input. The
//! local snapshot retains the original pitch range; cancellation keeps the last
//! finite viewport. No project command, history or released preview is owned here.
use super::{EditorView, midi_scene::SceneKey};
use gpui::{Context, Pixels, Size, Window};
use hex_widgets::{SplitterEvent, SplitterGesture, SplitterInput};

pub(super) struct Zoom {
    gesture: SplitterGesture,
    scene: SceneKey,
    applied_pitch: super::PitchViewport,
    viewport: Size<Pixels>,
    detail_height: f32,
    velocity_height: f32,
}
impl EditorView {
    pub(super) fn reconcile_piano_pitch_zoom(&mut self, window: &Window, cx: &mut Context<Self>) {
        let invalid = self.piano_pitch_zoom.as_ref().is_some_and(|z| {
            self.project_generation != z.scene.generation
                || self
                    .state
                    .as_ref()
                    .is_none_or(|s| s.revision != z.scene.revision)
                || self.detail_midi_clip_data() != Some(z.scene.clip)
                || self.piano_beat_start != z.scene.beat_start
                || self.piano_beat_span.raw() != z.scene.beat_span
                || self.piano_pitch != z.applied_pitch
                || !self.clip_view_open
                || self.device_panel_active
                || self.context_menu.is_some()
                || !self.midi_focus.is_focused(window)
                || !z.gesture.matches_window(window)
                || window.viewport_size() != z.viewport
                || self.clip_detail_body_height != z.detail_height
                || self.midi_velocity.visible_height() != z.velocity_height
        });
        if invalid {
            self.piano_pitch_zoom = None;
            cx.notify();
        }
    }
    pub(super) fn piano_pitch_input(&self, cx: &Context<Self>) -> SplitterInput<Self> {
        SplitterInput::new(
            cx.entity(),
            |v| v.piano_pitch_zoom.as_ref().map(|z| z.gesture),
            Self::piano_pitch_event,
        )
    }
    fn piano_pitch_event(
        &mut self,
        event: SplitterEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match event {
            SplitterEvent::Begin(gesture) => {
                if !self.clip_view_open || self.device_panel_active || self.context_menu.is_some() {
                    return false;
                }
                let Some(clip) = self.detail_midi_clip_data() else {
                    return false;
                };
                let Some(revision) = self.state.as_ref().map(|s| s.revision) else {
                    return false;
                };
                let pitch = self.piano_pitch;
                self.cancel_midi_note_gesture();
                self.midi_focus.focus(window);
                self.piano_pitch_zoom = Some(Zoom {
                    gesture,
                    scene: SceneKey {
                        generation: self.project_generation,
                        revision,
                        clip,
                        beat_start: self.piano_beat_start,
                        beat_span: self.piano_beat_span.raw(),
                        pitch,
                    },
                    applied_pitch: pitch,
                    viewport: window.viewport_size(),
                    detail_height: self.clip_detail_body_height,
                    velocity_height: self.midi_velocity.visible_height(),
                });
                cx.notify();
                true
            }
            SplitterEvent::Change(delta) | SplitterEvent::Commit(delta) => {
                self.reconcile_piano_pitch_zoom(window, cx);
                let Some(z) = self.piano_pitch_zoom.as_mut() else {
                    return false;
                };
                if !delta.is_finite() {
                    self.piano_pitch_zoom = None;
                    cx.notify();
                    return true;
                }
                let rows = (z.scene.pitch.span() as f32 * (delta / 120.).clamp(-8., 8.).exp2())
                    .round()
                    .clamp(1., 128.) as u8;
                let low = (z.scene.pitch.low()
                    + ((z.scene.pitch.span() - f64::from(rows)) / 2.0).trunc())
                .clamp(0.0, f64::from(128 - rows));
                let Some(pitch) = super::PitchViewport::new(low, f64::from(rows)) else {
                    return false;
                };
                // Preserve the original integer-row redraw threshold. The
                // window lease still consumes a sub-row move exclusively.
                if matches!(event, SplitterEvent::Change(_)) && self.piano_pitch == pitch {
                    return false;
                }
                self.piano_pitch = pitch;
                z.applied_pitch = pitch;
                if matches!(event, SplitterEvent::Commit(_)) {
                    self.piano_pitch_zoom = None;
                }
                cx.notify();
                true
            }
            SplitterEvent::Cancel(_) => {
                let changed = self.piano_pitch_zoom.take().is_some();
                if changed {
                    cx.notify();
                }
                changed
            }
        }
    }
}
