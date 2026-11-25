//! Local lane height through the existing shared splitter. Cancellation keeps
//! the last finite height; no project edit, history or released receipt exists.
use super::{EditorView, SceneKey};
use gpui::{Context, Window};
use hex_widgets::{SplitterEvent, SplitterGesture, SplitterInput};

pub(super) struct Resize {
    gesture: SplitterGesture,
    scene: SceneKey,
    height: f32,
}
impl EditorView {
    pub(in crate::editor) fn reconcile_velocity_size(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let invalid = self.midi_editor.velocity.resize.as_ref().is_some_and(|r| {
            r.scene.generation != self.project_generation
                || self
                    .state
                    .as_ref()
                    .is_none_or(|s| s.revision != r.scene.revision)
                || self.detail_midi_clip_data() != Some(r.scene.clip)
                || self.midi_editor.piano_beat_start != r.scene.start
                || self.midi_editor.piano_beat_span.raw() != r.scene.span
                || !self.audio_detail.clip_view_open
                || self.device_panel.active
                || !self.midi_editor.velocity.open
                || self.context_menu.is_some()
                || !self.midi_editor.focus.is_focused(window)
                || !r.gesture.matches_window(window)
        });
        if invalid {
            self.midi_editor.velocity.resize = None;
            cx.notify();
        }
    }
    pub(super) fn velocity_size_input(&self, cx: &Context<Self>) -> SplitterInput<Self> {
        SplitterInput::new(
            cx.entity(),
            |v| v.midi_editor.velocity.resize.as_ref().map(|r| r.gesture),
            Self::velocity_size_event,
        )
    }
    fn velocity_size_event(
        &mut self,
        event: SplitterEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match event {
            SplitterEvent::Begin(gesture) => {
                if !self.audio_detail.clip_view_open
                    || self.device_panel.active
                    || !self.midi_editor.velocity.open
                    || self.context_menu.is_some()
                    || !self.midi_editor.velocity.height.is_finite()
                {
                    return false;
                }
                let Some(clip) = self.detail_midi_clip_data() else {
                    return false;
                };
                let Some(revision) = self.state.as_ref().map(|s| s.revision) else {
                    return false;
                };
                self.cancel_midi_note_gesture();
                self.midi_editor.focus.focus(window);
                self.midi_editor.velocity.resize = Some(Resize {
                    gesture,
                    scene: SceneKey {
                        generation: self.project_generation,
                        revision,
                        clip,
                        start: self.midi_editor.piano_beat_start,
                        span: self.midi_editor.piano_beat_span.raw(),
                    },
                    height: self.midi_editor.velocity.height,
                });
                cx.notify();
                true
            }
            SplitterEvent::Change(delta) | SplitterEvent::Commit(delta) => {
                self.reconcile_velocity_size(window, cx);
                let Some(resize) = self.midi_editor.velocity.resize.as_ref() else {
                    return false;
                };
                let height = resize.height - delta;
                if !height.is_finite() {
                    self.midi_editor.velocity.resize = None;
                    cx.notify();
                    return true;
                }
                self.midi_editor.velocity.height = height.clamp(40., 128.);
                if matches!(event, SplitterEvent::Commit(_)) {
                    self.midi_editor.velocity.resize = None;
                }
                cx.notify();
                true
            }
            SplitterEvent::Cancel(_) => {
                let changed = self.midi_editor.velocity.resize.take().is_some();
                if changed {
                    cx.notify();
                }
                changed
            }
        }
    }
}
