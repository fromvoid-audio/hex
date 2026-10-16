//! Transient physical-key state; never edits the canonical project or history.

const NOTE_KEYS: [&str; 17] = [
    "a", "w", "s", "e", "d", "f", "t", "g", "y", "h", "u", "j", "k", "o", "l", "p", ";",
];
const VELOCITIES: [u8; 7] = hex_engine::computer_keyboard::COMPUTER_KEY_VELOCITIES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HeldKey {
    pub pitch: u8,
    pub velocity: u8,
}

pub(super) struct ComputerKeyboard {
    pub enabled: bool,
    /// MIDI 60 is middle C, displayed as c3 in Live's octave convention.
    pub base_pitch: u8,
    pub velocity: u8,
    held: [Option<HeldKey>; NOTE_KEYS.len()],
    tokens: [u64; NOTE_KEYS.len()],
    next_token: u64,
    pub input: Option<std::sync::Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>>,
    pub target: Option<(u64, u64, hex_project::TrackId)>,
    pub request_id: u64,
    pub suspended: bool,
}

impl Default for ComputerKeyboard {
    fn default() -> Self {
        Self {
            enabled: false,
            base_pitch: 60,
            velocity: 100,
            held: [None; NOTE_KEYS.len()],
            tokens: [0; NOTE_KEYS.len()],
            next_token: 0,
            input: None,
            target: None,
            request_id: 0,
            suspended: false,
        }
    }
}

impl ComputerKeyboard {
    /// Disable and lifecycle cancellation forget physical ownership together.
    pub fn release_all(&mut self) {
        self.held.fill(None);
    }

    pub fn toggle(&mut self) {
        self.enabled = !self.enabled;
        if !self.enabled {
            self.release_all();
        }
    }

    /// Returns whether an unmodified key belongs to the enabled keyboard.
    /// The caller rejects text-entry focus/modifiers before invoking this.
    pub fn key_down(&mut self, key: &str, is_held: bool) -> bool {
        if !self.enabled {
            return false;
        }
        if let Some(index) = NOTE_KEYS.iter().position(|candidate| *candidate == key) {
            if !is_held && self.held[index].is_none() {
                if let Some(pitch) = self
                    .base_pitch
                    .checked_add(index as u8)
                    .filter(|p| *p <= 127)
                {
                    let Some(token) = self
                        .next_token
                        .checked_add(1)
                        .filter(|token| *token <= u64::MAX >> 16)
                    else {
                        return true;
                    };
                    self.next_token = token;
                    self.tokens[index] = token;
                    self.held[index] = Some(HeldKey {
                        pitch,
                        velocity: self.velocity,
                    });
                }
            }
            return true;
        }
        if !matches!(key, "z" | "x" | "c" | "v") {
            return false;
        }
        if !is_held {
            match key {
                "z" => self.base_pitch = self.base_pitch.saturating_sub(12),
                "x" => self.base_pitch = self.base_pitch.saturating_add(12).min(120),
                "c" => {
                    let index = VELOCITIES
                        .iter()
                        .position(|value| *value == self.velocity)
                        .unwrap_or(4);
                    self.velocity = VELOCITIES[index.saturating_sub(1)];
                }
                "v" => {
                    let index = VELOCITIES
                        .iter()
                        .position(|value| *value == self.velocity)
                        .unwrap_or(4);
                    self.velocity = VELOCITIES[(index + 1).min(VELOCITIES.len() - 1)];
                }
                _ => {}
            }
        }
        true
    }

    /// Release by original physical key even after octave or modifier changes.
    pub fn key_up(&mut self, key: &str) -> bool {
        let key = if key == ":" { ";" } else { key };
        let Some(index) = NOTE_KEYS.iter().position(|candidate| *candidate == key) else {
            return false;
        };
        self.held[index].take().is_some()
    }

    #[cfg(test)]
    pub fn held(&self) -> &[Option<HeldKey>; NOTE_KEYS.len()] {
        &self.held
    }
}

impl super::EditorView {
    pub(super) fn cancel_computer_keyboard(&mut self) {
        if let Some(input) = self.computer_keyboard.input.take() {
            input.cancel();
        }
        self.computer_keyboard.release_all();
        self.computer_keyboard.target = None;
    }

    pub(super) fn toggle_computer_keyboard(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.cancel_computer_keyboard();
        self.computer_keyboard.toggle();
        self.computer_keyboard.suspended = false;
        self.focus.focus(window);
        self.tick_computer_keyboard();
        cx.notify();
    }

    pub(super) fn tick_computer_keyboard(&mut self) -> bool {
        if !self.computer_keyboard.enabled || self.computer_keyboard.suspended {
            return false;
        }
        if self.device_gesture_pending()
            || self.prepared_seek.is_some()
            || self.stopped_playback_restore.is_some()
        {
            self.cancel_computer_keyboard();
            self.computer_keyboard.suspended = true;
            return true;
        }
        let target = self.state.as_ref().and_then(|state| {
            self.selected_track
                .map(|track| (self.project_generation, state.revision, track))
        });
        let cancelled = self
            .computer_keyboard
            .input
            .as_ref()
            .is_some_and(|input| input.is_cancelled());
        if cancelled {
            self.cancel_computer_keyboard();
            self.computer_keyboard.suspended = true;
            return true;
        }
        if target == self.computer_keyboard.target && !cancelled {
            if self
                .computer_keyboard
                .input
                .as_ref()
                .is_some_and(|input| input.rejected())
            {
                let message = "computer keyboard note refused · unsupported pitch or voice capacity · release keys and toggle keyboard to retry";
                if self.error.as_deref() != Some(message) {
                    self.error = Some(message.into());
                    return true;
                }
                return false;
            }
            return false;
        }
        self.cancel_computer_keyboard();
        let Some((generation, revision, track)) = target else {
            return false;
        };
        self.computer_keyboard.target = target;
        if self
            .state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .is_none_or(|track| track.instrument().is_none_or(|device| device.bypassed()))
        {
            self.error =
                Some("select a track with an enabled instrument for the computer keyboard".into());
            return true;
        }
        let Some(request_id) = self.computer_keyboard.request_id.checked_add(1) else {
            self.error = Some("computer keyboard request identity exhausted".into());
            return true;
        };
        self.computer_keyboard.request_id = request_id;
        let input =
            std::sync::Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
        self.computer_keyboard.input = Some(std::sync::Arc::clone(&input));
        if !self.enqueue(hex_session::service::Request::PrepareComputerKeyboard {
            project_generation: generation,
            revision,
            request_id,
            track,
            input,
        }) {
            self.cancel_computer_keyboard();
            self.computer_keyboard.suspended = true;
        }
        true
    }

    pub(super) fn computer_keyboard_accepts(
        &self,
        key: &str,
        modifiers: gpui::Modifiers,
        window: &gpui::Window,
        _cx: &gpui::App,
    ) -> bool {
        !modifiers.modified()
            && !self.theme_settings.open
            && self.context_menu.is_none()
            && !self.opening_project
            && !self.close_prompt_open
            && (self.focus.is_focused(window) || self.midi_focus.is_focused(window))
            && (key == "m"
                || (self.computer_keyboard.enabled
                    && (NOTE_KEYS.contains(&key) || matches!(key, "z" | "x" | "c" | "v"))))
    }

    pub(super) fn computer_keyboard_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        if !self.computer_keyboard_accepts(key, event.keystroke.modifiers, window, cx) {
            return;
        }
        if key == "m" {
            if !event.is_held {
                self.toggle_computer_keyboard(window, cx);
            }
        } else {
            if self.device_gesture_pending()
                || self.prepared_seek.is_some()
                || self.stopped_playback_restore.is_some()
                || (self.computer_keyboard.suspended && event.is_held)
            {
                self.cancel_computer_keyboard();
                self.computer_keyboard.suspended = true;
                cx.stop_propagation();
                return;
            }
            self.computer_keyboard.suspended = false;
            self.tick_computer_keyboard();
            if self.computer_keyboard.key_down(key, event.is_held) {
                self.publish_computer_keyboard();
            }
            cx.notify();
        }
        cx.stop_propagation();
    }

    pub(super) fn computer_keyboard_up(&mut self, key: &str) -> bool {
        let released = self.computer_keyboard.key_up(key);
        if released {
            self.publish_computer_keyboard();
        }
        released
    }

    fn publish_computer_keyboard(&mut self) {
        let Some(input) = &self.computer_keyboard.input else {
            return;
        };
        for (key, held) in self.computer_keyboard.held.iter().enumerate() {
            if let Err(error) = input.set_key(
                key,
                held.map(|held| {
                    (
                        held.pitch,
                        held.velocity,
                        self.computer_keyboard.tokens[key],
                    )
                }),
            ) {
                self.error = Some(error.to_string());
                input.cancel();
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_default_and_complete_chromatic_mapping() {
        let mut keyboard = ComputerKeyboard::default();
        assert!(!keyboard.key_down("a", false));
        assert!(keyboard.held().iter().all(Option::is_none));
        keyboard.toggle();
        for (index, key) in NOTE_KEYS.iter().enumerate() {
            assert!(keyboard.key_down(key, false));
            assert_eq!(
                keyboard.held()[index],
                Some(HeldKey {
                    pitch: 60 + index as u8,
                    velocity: 100
                })
            );
        }
        keyboard.toggle();
        assert!(keyboard.held().iter().all(Option::is_none));
    }

    #[test]
    fn repeat_does_not_retrigger_and_key_up_keeps_original_pitch() {
        let mut keyboard = ComputerKeyboard::default();
        keyboard.toggle();
        keyboard.key_down("a", false);
        keyboard.key_down("x", false);
        keyboard.key_down("v", false);
        keyboard.key_down("a", true);
        keyboard.key_down("a", false);
        assert_eq!(
            keyboard.held()[0],
            Some(HeldKey {
                pitch: 60,
                velocity: 100
            })
        );
        assert!(keyboard.key_up("a"));
        assert!(!keyboard.key_up("a"));
        keyboard.key_down("a", false);
        assert_eq!(
            keyboard.held()[0],
            Some(HeldKey {
                pitch: 72,
                velocity: 120
            })
        );
    }

    #[test]
    fn controls_are_bounded_and_repeats_do_not_shift() {
        let mut keyboard = ComputerKeyboard::default();
        keyboard.toggle();
        keyboard.key_down("z", true);
        assert_eq!(keyboard.base_pitch, 60);
        for _ in 0..20 {
            keyboard.key_down("z", false);
            keyboard.key_down("c", false);
        }
        assert_eq!((keyboard.base_pitch, keyboard.velocity), (0, 20));
        for _ in 0..20 {
            keyboard.key_down("x", false);
            keyboard.key_down("v", false);
        }
        assert_eq!((keyboard.base_pitch, keyboard.velocity), (120, 127));
        keyboard.key_down("c", false);
        assert_eq!(keyboard.velocity, 120);
        keyboard.key_down("c", false);
        assert_eq!(keyboard.velocity, 100);
        keyboard.key_down(";", false);
        assert!(keyboard.held()[16].is_none());
        keyboard.key_down("a", false);
        assert_eq!(keyboard.held()[0].map(|note| note.pitch), Some(120));
        keyboard.release_all();
        assert!(keyboard.held().iter().all(Option::is_none));
        assert!(!keyboard.key_down("b", false));
    }
}
