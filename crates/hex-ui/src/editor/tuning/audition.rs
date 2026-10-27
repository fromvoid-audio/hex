use super::*;
use std::sync::Arc;

impl TuningState {
    pub(super) fn degree_activity(&self, degree: usize) -> (bool, bool) {
        let scale = self.value.scale();
        let mut held = false;
        let mut recent = false;
        for key in 0..128 {
            let mapped = (key as i32 - i32::from(scale.root_key()) + scale.root_degree() as i32)
                .rem_euclid(scale.count() as i32) as usize;
            if mapped == degree {
                held |= (self.held_keys[key / 64] | self.midi_keys[key / 64])
                    & (1_u64 << (key % 64))
                    != 0;
                recent |= self.recent_keys[key].is_some();
            }
        }
        (held, recent)
    }
}
impl EditorView {
    pub(in crate::editor) fn tuning_audition_accepts(
        &self,
        key: &str,
        modifiers: gpui::Modifiers,
        window: &Window,
    ) -> bool {
        self.tuning.open
            && self.tuning.focus.is_focused(window)
            && !modifiers.modified()
            && !self.opening_project
            && !self.close_prompt_open
            && self.tuning.audition_keyboard.accepts_key(key)
    }
    pub(in crate::editor) fn cancel_tuning_audition(&mut self) {
        self.tuning.audition_preparing = false;
        if let Some(input) = self.tuning.audition_input.take() {
            input.cancel();
        }
        self.tuning.audition_keyboard.release_all();
        self.tuning.held_keys = [0; 2];
        self.tuning.midi_keys = [0; 2];
        self.tuning.midi_last_query = None;
        self.tuning.recent_keys = [None; 128];
    }
    pub(in crate::editor) fn tuning_audition_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.tuning_audition_accepts(&event.keystroke.key, event.keystroke.modifiers, window) {
            return false;
        }
        if !self
            .tuning
            .audition_keyboard
            .key_down(&event.keystroke.key, event.is_held)
        {
            return false;
        }
        if self
            .tuning
            .audition_input
            .as_ref()
            .is_none_or(|input| input.is_cancelled())
        {
            // Autorepeat after Stop/focus loss must not revive a held key.
            if event.is_held {
                return true;
            }
            let Some(revision) = self.state.as_ref().map(|state| state.revision) else {
                return true;
            };
            let input = Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(
                self.project_generation,
            ));
            if self.enqueue(Request::PrepareTuningAudition {
                revision,
                input: Arc::clone(&input),
            }) {
                self.tuning.audition_input = Some(input);
                self.tuning.audition_preparing = true;
            } else {
                input.cancel();
                self.tuning.error = Some("audition preparation queue is full".into());
            }
        }
        self.publish_tuning_audition();
        cx.stop_propagation();
        cx.notify();
        true
    }
    pub(in crate::editor) fn tuning_audition_up(&mut self, key: &str) -> bool {
        let released = self.tuning.audition_keyboard.key_up(key);
        if released {
            self.publish_tuning_audition();
        }
        released
    }
    fn publish_tuning_audition(&mut self) {
        if let Some(input) = &self.tuning.audition_input {
            for (slot, note) in self
                .tuning
                .audition_keyboard
                .desired_notes()
                .into_iter()
                .enumerate()
            {
                if let Err(error) = input.set_key(slot, note) {
                    input.cancel();
                    self.tuning.error = Some(error.to_string());
                    break;
                }
            }
        }
    }
    pub(in crate::editor) fn tick_tuning_audition(&mut self) -> bool {
        let Some(input) = self.tuning.audition_input.as_ref() else {
            return false;
        };
        if input.is_cancelled() {
            // Worker refusal cancels before sending its completion. Keep this
            // owner until receive consumes that completion, even across ticks.
            // Explicit Stop/focus/close cancellation takes the owner immediately.
            if self.tuning.audition_preparing {
                return false;
            }
            self.cancel_tuning_audition();
            return true;
        }
        let held = input.held_keys();
        let now = Instant::now();
        let mut changed = held != self.tuning.held_keys;
        for key in 0..128 {
            if held[key / 64] & (1_u64 << (key % 64)) != 0 {
                self.tuning.recent_keys[key] = Some(now);
            } else if self.tuning.recent_keys[key]
                .is_some_and(|at| now.duration_since(at) > Duration::from_millis(400))
            {
                self.tuning.recent_keys[key] = None;
                changed = true;
            }
        }
        self.tuning.held_keys = held;
        if input.take_rejection()
            && self.tuning.error.as_deref() != Some("audition note refused · pitch or voice limit")
        {
            self.tuning.error = Some("audition note refused · pitch or voice limit".into());
            changed = true;
        }
        changed
    }
}
