//! Transient physical-key state; never edits the canonical project or history.

pub(super) const NOTE_KEYS: [&str; 17] = [
    "a", "w", "s", "e", "d", "f", "t", "g", "y", "h", "u", "j", "k", "o", "l", "p", ";",
];
const VELOCITIES: [u8; 7] = hex_engine::computer_keyboard::COMPUTER_KEY_VELOCITIES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) struct HeldKey {
    pub pitch: u8,
    pub velocity: u8,
}

pub(in crate::editor) struct ComputerKeyboard {
    pub enabled: bool,
    /// MIDI 60 is middle C, displayed as c3 in Live's octave convention.
    pub base_pitch: u8,
    pub velocity: u8,
    pub(super) held: [Option<HeldKey>; NOTE_KEYS.len()],
    /// Physical ownership for editor consumers; audio lease retirement is separate.
    physical: [Option<HeldKey>; NOTE_KEYS.len()],
    pub(super) tokens: [u64; NOTE_KEYS.len()],
    next_token: u64,
    pub input: Option<std::sync::Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>>,
    pub target: Option<(u64, u64, hex_project::TrackId)>,
    pub request_id: u64,
    pub prepared: bool,
    pub suspended: bool,
}

impl Default for ComputerKeyboard {
    fn default() -> Self {
        Self {
            enabled: false,
            base_pitch: 60,
            velocity: 100,
            held: [None; NOTE_KEYS.len()],
            physical: [None; NOTE_KEYS.len()],
            tokens: [0; NOTE_KEYS.len()],
            next_token: 0,
            input: None,
            target: None,
            request_id: 0,
            prepared: false,
            suspended: false,
        }
    }
}

impl ComputerKeyboard {
    pub(in crate::editor) fn accepts_key(&self, key: &str) -> bool {
        self.enabled && (NOTE_KEYS.contains(&key) || matches!(key, "z" | "x" | "c" | "v"))
    }
    pub(in crate::editor) fn desired_notes(&self) -> [Option<(u8, u8, u64)>; 17] {
        std::array::from_fn(|slot| {
            self.held[slot].map(|note| (note.pitch, note.velocity, self.tokens[slot]))
        })
    }
    /// Disable and lifecycle cancellation forget physical ownership together.
    pub fn release_all(&mut self) {
        self.held.fill(None);
        self.physical.fill(None);
    }

    pub(in crate::editor) fn physical_notes(&self) -> impl Iterator<Item = HeldKey> + '_ {
        self.physical.iter().copied().flatten()
    }

    pub(in crate::editor) fn physical_through(&self) -> u64 {
        self.next_token
    }

    pub(in crate::editor) fn physical_notes_after(
        &self,
        floor: u64,
    ) -> impl Iterator<Item = HeldKey> + '_ {
        self.physical
            .iter()
            .zip(self.tokens)
            .filter_map(move |(note, token)| (token > floor).then_some(*note).flatten())
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
            if !is_held
                && self.physical[index].is_none()
                && let Some(pitch) = self
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
                let held = HeldKey {
                    pitch,
                    velocity: self.velocity,
                };
                self.held[index] = Some(held);
                self.physical[index] = Some(held);
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
        let active = self.held[index].take().is_some();
        self.physical[index].take().is_some() || active
    }

    #[cfg(test)]
    pub fn held(&self) -> &[Option<HeldKey>; NOTE_KEYS.len()] {
        &self.held
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;

    #[test]
    fn display_floor_excludes_old_keys_without_releasing_physical_ownership() {
        let mut keyboard = ComputerKeyboard::default();
        keyboard.toggle();
        keyboard.key_down("a", false);
        let floor = keyboard.physical_through();
        keyboard.key_down("a", true);
        keyboard.key_down("d", false);
        assert_eq!(keyboard.physical_notes().count(), 2);
        assert_eq!(
            keyboard
                .physical_notes_after(floor)
                .map(|n| n.pitch)
                .collect::<Vec<_>>(),
            [64]
        );
        keyboard.key_up("d");
        assert!(keyboard.physical_notes_after(floor).next().is_none());
        keyboard.key_up("a");
        keyboard.key_down("a", false);
        assert_eq!(
            keyboard
                .physical_notes_after(floor)
                .map(|n| n.pitch)
                .collect::<Vec<_>>(),
            [60]
        );
    }

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
