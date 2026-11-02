//! A bounded physical-note snapshot, separate from audio monitoring or recording.
//! One native port callback writes; control reads at most two coherent attempts.
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering::SeqCst};

pub const HELD_NOTE_COUNT: usize = 128;
const MAX_TOKEN: u64 = u64::MAX >> 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldNote {
    pub pitch: u8,
    pub velocity: u8,
    pub token: u64,
}

/// Tokens belong to one connection activation. A control-side floor excludes
/// keys pressed before Stop/focus/target discontinuities without reviving them
/// when an unrelated new NoteOn publishes the next complete snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldNotes {
    words: [u64; HELD_NOTE_COUNT],
    through: u64,
}
impl Default for HeldNotes {
    fn default() -> Self {
        Self {
            words: [0; HELD_NOTE_COUNT],
            through: 0,
        }
    }
}
impl HeldNotes {
    pub fn through(&self) -> u64 {
        self.through
    }
    pub fn after(&self, floor: u64) -> impl Iterator<Item = HeldNote> + '_ {
        self.words
            .iter()
            .copied()
            .enumerate()
            .filter_map(move |(pitch, word)| {
                let token = word >> 8;
                (word != 0 && token > floor).then_some(HeldNote {
                    pitch: pitch as u8,
                    velocity: word as u8,
                    token,
                })
            })
    }
    pub(crate) fn clear(&mut self) {
        self.words.fill(0);
    }
    pub(crate) fn observe(&mut self, message: &[u8], channel: u8) -> Result<bool, HeldNoteFault> {
        let [status, pitch, velocity] = message else {
            return Ok(false);
        };
        if channel >= 16 || status & 15 != channel || *pitch > 127 || *velocity > 127 {
            return Ok(false);
        }
        match status & 0xf0 {
            0x90 if *velocity != 0 => {
                let next = self
                    .through
                    .checked_add(1)
                    .filter(|value| *value <= MAX_TOKEN)
                    .ok_or(HeldNoteFault::TokenLimit)?;
                self.through = next;
                self.words[usize::from(*pitch)] = (next << 8) | u64::from(*velocity);
                Ok(true)
            }
            0x80 | 0x90 => {
                let word = &mut self.words[usize::from(*pitch)];
                let changed = *word != 0;
                *word = 0;
                Ok(changed)
            }
            0xb0 if matches!(*pitch, 120 | 123) => {
                let changed = self.words.iter().any(|word| *word != 0);
                self.clear();
                Ok(changed)
            }
            _ => Ok(false),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HeldNoteFault {
    TokenLimit = 2,
    PublicationLimit = 3,
}
// The fixed 1 KiB stack snapshot keeps polling allocation-free; boxing would
// allocate on each coherent read merely to shrink the empty result variants.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldNotesRead {
    Snapshot(HeldNotes),
    Busy,
    Cancelled,
    Fault(HeldNoteFault),
}

/// Sole-writer port endpoint. Terminal cancellation overrides late publications.
/// No allocation, waiting or retry loop occurs on publication. Native MIDI work
/// is not an audio-callback or physical-device qualification claim.
pub struct HeldNoteInput {
    sequence: AtomicU64,
    words: [AtomicU64; HELD_NOTE_COUNT],
    through: AtomicU64,
    cancelled: AtomicBool,
    fault: AtomicU8,
}
impl Default for HeldNoteInput {
    fn default() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
            through: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
            fault: AtomicU8::new(0),
        }
    }
}
impl HeldNoteInput {
    pub fn cancel(&self) {
        self.cancelled.store(true, SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(SeqCst)
    }
    pub fn fail(&self, fault: HeldNoteFault) {
        self.fault.store(fault as u8, SeqCst);
        self.cancel();
    }
    pub(crate) fn publish(&self, notes: HeldNotes) {
        if self.is_cancelled() {
            return;
        }
        let before = self.sequence.load(SeqCst);
        // The low bit marks an in-flight publication.
        let Some(after) = before.checked_add(2).filter(|_| before & 1 == 0) else {
            self.fail(HeldNoteFault::PublicationLimit);
            return;
        };
        self.sequence.store(before + 1, SeqCst);
        for (target, word) in self.words.iter().zip(notes.words) {
            target.store(word, SeqCst);
        }
        self.through.store(notes.through, SeqCst);
        self.sequence.store(after, SeqCst);
    }
    pub fn read(&self) -> HeldNotesRead {
        for _ in 0..2 {
            match self.fault.load(SeqCst) {
                2 => return HeldNotesRead::Fault(HeldNoteFault::TokenLimit),
                3 => return HeldNotesRead::Fault(HeldNoteFault::PublicationLimit),
                _ => {}
            }
            if self.is_cancelled() {
                return HeldNotesRead::Cancelled;
            }
            let before = self.sequence.load(SeqCst);
            if before & 1 != 0 {
                continue;
            }
            let notes = HeldNotes {
                words: std::array::from_fn(|i| self.words[i].load(SeqCst)),
                through: self.through.load(SeqCst),
            };
            if before == self.sequence.load(SeqCst) && !self.is_cancelled() {
                return HeldNotesRead::Snapshot(notes);
            }
        }
        if self.is_cancelled() {
            HeldNotesRead::Cancelled
        } else {
            HeldNotesRead::Busy
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Controllers, Decoder, deliver_packets};
    fn feed(decoder: &mut Decoder, input: &HeldNoteInput, bytes: &[u8]) -> HeldNotes {
        deliver_packets(decoder, 2, Controllers::default(), [bytes], input, &|_| {});
        let HeldNotesRead::Snapshot(snapshot) = input.read() else {
            panic!("coherent snapshot")
        };
        snapshot
    }
    #[test]
    fn running_status_exact_velocities_release_channels_and_controller_reset() {
        let input = HeldNoteInput::default();
        let mut decoder = Decoder::default();
        let one = feed(&mut decoder, &input, &[0x92, 60, 37, 64]);
        assert_eq!(
            one.after(0)
                .map(|n| (n.pitch, n.velocity))
                .collect::<Vec<_>>(),
            [(60, 37)]
        );
        let two = feed(&mut decoder, &input, &[0xf8, 93, 0x93, 69, 88, 0x92, 60, 0]);
        assert_eq!(
            two.after(0)
                .map(|n| (n.pitch, n.velocity))
                .collect::<Vec<_>>(),
            [(64, 93)]
        );
        // Reset controllers does not release physical keys. Expression remains usable.
        let current = deliver_packets(
            &mut decoder,
            2,
            Controllers {
                pressure: 77,
                bend: 200,
                wheel: 13,
            },
            [&[0xb2, 121, 0][..]],
            &input,
            &|_| {},
        );
        assert_eq!(current, Controllers::default());
        assert_eq!(input.read(), HeldNotesRead::Snapshot(two));
        let empty = feed(&mut decoder, &input, &[0x82, 64, 127]);
        assert_eq!(empty.after(0).count(), 0);
        assert!(
            feed(&mut decoder, &input, &[0x92, 60, 93, 0xb2, 123, 0])
                .after(0)
                .next()
                .is_none()
        );
        assert!(
            feed(&mut decoder, &input, &[0x92, 60, 93, 0xff])
                .after(0)
                .next()
                .is_none()
        );
    }
    #[test]
    fn token_floor_does_not_revive_old_keys_after_an_unrelated_new_press() {
        let input = HeldNoteInput::default();
        let mut decoder = Decoder::default();
        let floor = feed(&mut decoder, &input, &[0x92, 60, 37]).through();
        let later = feed(&mut decoder, &input, &[64, 93]);
        assert_eq!(
            later.after(floor).map(|n| n.pitch).collect::<Vec<_>>(),
            [64]
        );
        let renewed = feed(&mut decoder, &input, &[60, 0, 60, 88, 64, 0]);
        assert_eq!(
            renewed
                .after(floor)
                .map(|n| (n.pitch, n.velocity))
                .collect::<Vec<_>>(),
            [(60, 88)]
        );
    }
    #[test]
    fn coherent_busy_read_is_bounded_and_cancellation_dominates_late_publication() {
        let input = HeldNoteInput::default();
        let mut decoder = Decoder::default();
        let snapshot = feed(&mut decoder, &input, &[0x92, 60, 37, 64, 93]);
        input.sequence.store(3, SeqCst);
        assert_eq!(input.read(), HeldNotesRead::Busy);
        input.sequence.store(2, SeqCst);
        assert_eq!(input.read(), HeldNotesRead::Snapshot(snapshot));
        input.cancel();
        input.publish(snapshot);
        assert_eq!(input.read(), HeldNotesRead::Cancelled);
    }
    #[test]
    fn malformed_sysex_and_wrong_channel_cannot_create_phantom_notes() {
        let input = HeldNoteInput::default();
        let mut decoder = Decoder::default();
        for bytes in [
            &[60, 37][..],
            &[0x93, 60, 37],
            &[0xf0, 0x01, 60, 37, 0xf7],
            &[0x92, 60, 0xf1, 37],
            &[0xc2, 60, 37],
        ] {
            assert_eq!(feed(&mut decoder, &input, bytes).after(0).count(), 0);
        }
    }
    #[test]
    fn token_and_publication_limits_fail_closed() {
        let tokens = HeldNoteInput::default();
        let mut decoder = Decoder::default();
        decoder.held.through = MAX_TOKEN;
        deliver_packets(
            &mut decoder,
            2,
            Controllers::default(),
            [&[0x92, 60, 37][..]],
            &tokens,
            &|_| {},
        );
        assert_eq!(
            tokens.read(),
            HeldNotesRead::Fault(HeldNoteFault::TokenLimit)
        );
        let publications = HeldNoteInput::default();
        publications.sequence.store(u64::MAX - 1, SeqCst);
        publications.publish(HeldNotes::default());
        assert_eq!(
            publications.read(),
            HeldNotesRead::Fault(HeldNoteFault::PublicationLimit)
        );
    }
}
