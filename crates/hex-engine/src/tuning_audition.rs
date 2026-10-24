//! Transient reference-tone input and audio-owned fixed-capacity synthesis.
//! No project track, plugin, MTS library or offline export registration is involved.
use crate::{Frame, Invalid};
use hex_devices::tuning_audition::{PreparedAudition, Synth, Waveform};
use std::sync::{
    Arc,
    atomic::{
        AtomicBool, AtomicU64,
        Ordering::{Acquire, Release},
    },
};

pub const KEY_SLOTS: usize = 17;
/// Existing fixed monitor ceiling, reserved by control before attachment.
pub const MAX_TUNING_AUDITION_STORAGE_BYTES: usize = 8192;

/// One project-generation-scoped activation. Cancellation bypasses the worker
/// queue and is terminal; a new editor activation needs a fresh input owner.
#[derive(Debug)]
pub struct TuningAuditionInput {
    pub project_generation: u64,
    notes: [AtomicU64; KEY_SLOTS],
    held: [AtomicU64; 2],
    cancelled: AtomicBool,
    rejected: AtomicBool,
}

impl TuningAuditionInput {
    pub fn new(project_generation: u64) -> Self {
        Self {
            project_generation,
            notes: std::array::from_fn(|_| AtomicU64::new(0)),
            held: std::array::from_fn(|_| AtomicU64::new(0)),
            cancelled: AtomicBool::new(false),
            rejected: AtomicBool::new(false),
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Acquire)
    }
    /// Consume one callback refusal notification without latching future UI errors.
    pub fn take_rejection(&self) -> bool {
        self.rejected
            .swap(false, std::sync::atomic::Ordering::AcqRel)
    }
    pub fn held_keys(&self) -> [u64; 2] {
        self.held.each_ref().map(|word| word.load(Acquire))
    }
    /// Each physical input slot carries a monotonically unique, nonzero token.
    /// Releasing an old token can never release a replacement voice.
    pub fn set_key(&self, slot: usize, note: Option<(u8, u8, u64)>) -> Result<(), Invalid> {
        let destination = self
            .notes
            .get(slot)
            .ok_or(Invalid("audition input slot is out of range"))?;
        let word = match note {
            None => 0,
            Some((key, velocity, token))
                if key < 128
                    && (1..=127).contains(&velocity)
                    && token > 0
                    && token <= u64::MAX >> 16 =>
            {
                (token << 16) | (u64::from(velocity) << 8) | u64::from(key + 1)
            }
            _ => return Err(Invalid("invalid audition note or owner")),
        };
        destination.store(word, Release);
        Ok(())
    }
}

pub(crate) struct Runtime {
    input: Arc<TuningAuditionInput>,
    prepared: PreparedAudition,
    synth: Synth,
    desired: [u64; KEY_SLOTS],
    detached: bool,
}
impl Runtime {
    pub(crate) fn prepare(
        project: &hex_project::Project,
        input: Arc<TuningAuditionInput>,
    ) -> Result<Self, Invalid> {
        let tuning = project.tuning();
        let settings = tuning.audition();
        let waveform = match settings.waveform() {
            hex_lattice::TuningWaveform::Triangle => Waveform::Triangle,
            hex_lattice::TuningWaveform::Square => Waveform::Square,
            hex_lattice::TuningWaveform::Saw => Waveform::Saw,
        };
        // Audition intentionally hears the working scale even while project-wide
        // application is disabled. Opening the editor never enables/edits tuning.
        let prepared = PreparedAudition::new(
            project.sample_rate(),
            tuning.scale().frequencies(),
            waveform,
            settings.level(),
            settings.attack_ms(),
            settings.release_ms(),
        )
        .map_err(|_| Invalid("invalid tuning audition preparation"))?;
        Ok(Self {
            input,
            prepared,
            synth: Synth::new(prepared),
            desired: [0; KEY_SLOTS],
            detached: false,
        })
    }
    pub(crate) fn storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of::<TuningAuditionInput>()
            + 2 * std::mem::size_of::<usize>()
    }
    pub(crate) fn shares_live_lease(&self, old: &Self) -> bool {
        !old.detached && !self.input.is_cancelled() && Arc::ptr_eq(&self.input, &old.input)
    }
    pub(crate) fn can_rebind_input_from(&self, old: &Self) -> bool {
        self.shares_live_lease(old)
            && !self.detached
            && self.synth.active_voices() == 0
            && self.desired.iter().all(|word| *word == 0)
    }
    pub(crate) fn detach_rebound_input(&mut self) {
        self.detached = true;
    }
    pub(crate) fn continued_synth(&self, old: &Self) -> Result<Synth, Invalid> {
        let mut scratch = self.synth;
        scratch
            .retarget_from(&old.synth, self.prepared)
            .map_err(|_| Invalid("audition sample rate cannot be continued"))?;
        Ok(scratch)
    }
    pub(crate) fn preserve_from(&mut self, old: &mut Self, continued: Synth) {
        if continued.active_voices() < old.synth.active_voices() {
            self.input.rejected.store(true, Release);
        }
        self.synth = continued;
        self.desired = old.desired;
        // A refused physical input has no voice to carry. Retry it once under
        // the new table; preserve cached owners that really continued.
        for desired in &mut self.desired {
            if *desired != 0 && !self.synth.owns_voice(*desired >> 16) {
                *desired = 0;
            }
        }
        old.detached = true;
    }
    pub(crate) fn cancel(&mut self) {
        if !self.detached {
            self.input.cancel();
        }
        self.synth.clear();
        if !self.detached {
            for word in &self.input.held {
                word.store(0, Release);
            }
        }
    }
    pub(crate) fn mix(&mut self, output: &mut [Frame]) {
        if self.detached || self.input.is_cancelled() {
            self.cancel();
            return;
        }
        for (slot, desired) in self.desired.iter_mut().enumerate() {
            let next = self.input.notes[slot].load(Acquire);
            if next == *desired {
                continue;
            }
            if *desired != 0 {
                self.synth.note_off(*desired >> 16);
            }
            if next != 0 {
                let key = ((next & 255) - 1) as u8;
                let velocity = ((next >> 8) & 255) as f32 / 127.0;
                if !self.synth.has_held_voice_capacity()
                    || self.synth.note_on(key, velocity, next >> 16).is_err()
                {
                    self.input.rejected.store(true, Release);
                }
            }
            *desired = next;
        }
        for frame in output {
            let tone = self.synth.next_sample();
            frame[0] += tone;
            frame[1] += tone;
        }
        for (word, mask) in self.input.held.iter().zip(self.synth.held_key_mask()) {
            word.store(mask, Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(root_key: u8, hz: f64) -> hex_project::Project {
        let mut snapshot = hex_project::Project::new(48_000).unwrap().snapshot();
        let mut scale = hex_tuning::Scale::equal(12);
        scale.set_reference(root_key, hz, 0).unwrap();
        snapshot.tuning =
            hex_tuning::ProjectTuning::new(false, scale, hex_tuning::Audition::default()).unwrap();
        hex_project::Project::from_snapshot(snapshot).unwrap()
    }

    #[test]
    fn review_final_unsupported_reference_voices_retire_without_rejecting_retuning() {
        for releasing in [false, true] {
            let input = Arc::new(TuningAuditionInput::new(0));
            let mut source = Runtime::prepare(&project(60, 220.0), Arc::clone(&input)).unwrap();
            input.set_key(0, Some((60, 100, 1))).unwrap();
            input.set_key(1, Some((0, 100, 2))).unwrap();
            source.mix(&mut [[0.0; 2]; 128]);
            if releasing {
                input.set_key(0, None).unwrap();
                source.mix(&mut [[0.0; 2]; 128]);
            }
            let mut target = Runtime::prepare(&project(0, 20_000.0), Arc::clone(&input)).unwrap();
            let continued = target.continued_synth(&source).unwrap();
            target.preserve_from(&mut source, continued);
            let mut pcm = [[0.0; 2]; 128];
            target.mix(&mut pcm);
            assert_eq!(input.held_keys(), [1, 0]);
            assert!(pcm.iter().any(|frame| frame[0] != 0.0));
            assert!(input.take_rejection());
            assert!(!input.is_cancelled());
            if !releasing {
                let mut valid = Runtime::prepare(&project(60, 220.0), Arc::clone(&input)).unwrap();
                let continued = valid.continued_synth(&target).unwrap();
                valid.preserve_from(&mut target, continued);
                valid.mix(&mut [[0.0; 2]; 128]);
                assert_eq!(input.held_keys(), [(1 << 60) | 1, 0]);
            }
        }
    }

    #[test]
    fn review_followup_excess_audition_note_refuses_without_stealing_held_owners() {
        let input = Arc::new(TuningAuditionInput::new(0));
        let mut runtime = Runtime::prepare(&project(60, 220.0), Arc::clone(&input)).unwrap();
        for slot in 0..KEY_SLOTS {
            input
                .set_key(slot, Some((48 + slot as u8, 100, slot as u64 + 1)))
                .unwrap();
        }
        runtime.mix(&mut [[0.0; 2]; 128]);
        assert!(
            input.take_rejection(),
            "the excess physical note must be explicitly refused"
        );
        assert_eq!(input.held_keys(), [0xffff_u64 << 48, 0]);
        input.set_key(0, None).unwrap();
        input.set_key(16, None).unwrap();
        runtime.mix(&mut [[0.0; 2]; 128]);
        input.set_key(16, Some((64, 100, 100))).unwrap();
        runtime.mix(&mut [[0.0; 2]; 128]);
        assert_eq!(input.held_keys(), [0xfffe_u64 << 48, 1]);
        assert!(!input.take_rejection());
    }

    #[test]
    fn review_highest_midi_key_encodes_renders_and_releases_without_sentinel_collision() {
        let input = Arc::new(TuningAuditionInput::new(0));
        let mut runtime = Runtime::prepare(&project(127, 220.0), Arc::clone(&input)).unwrap();
        input.set_key(0, Some((127, 100, 1))).unwrap();
        let mut output = [[0.0; 2]; 128];
        runtime.mix(&mut output);
        assert_eq!(input.held_keys(), [0, 1 << 63]);
        assert!(output.iter().any(|frame| frame[0] != 0.0));
        assert!(!input.take_rejection());
        input.set_key(0, None).unwrap();
        runtime.mix(&mut output);
        assert_eq!(input.held_keys(), [0; 2]);
    }

    #[test]
    fn review_audition_rejection_is_reported_once_and_supported_notes_recover() {
        let input = Arc::new(TuningAuditionInput::new(0));
        let mut runtime = Runtime::prepare(&project(60, 20_000.0), Arc::clone(&input)).unwrap();
        input.set_key(0, Some((127, 100, 1))).unwrap();
        runtime.mix(&mut [[0.0; 2]; 64]);
        assert!(input.take_rejection());
        input.set_key(0, Some((0, 100, 2))).unwrap();
        let mut output = [[0.0; 2]; 128];
        runtime.mix(&mut output);
        assert_eq!(input.held_keys(), [1, 0]);
        assert!(output.iter().any(|frame| frame[0] != 0.0));
        assert!(
            !input.take_rejection(),
            "reported refusal must not remain latched"
        );
    }
    #[test]
    fn review_held_rejected_audition_retries_when_retune_makes_pitch_valid() {
        let input = Arc::new(TuningAuditionInput::new(0));
        let mut source = Runtime::prepare(&project(60, 20_000.0), Arc::clone(&input)).unwrap();
        input.set_key(0, Some((127, 100, 1))).unwrap();
        source.mix(&mut [[0.0; 2]; 64]);
        assert!(input.take_rejection());
        assert_eq!(input.held_keys(), [0; 2]);
        let mut target = Runtime::prepare(&project(127, 220.0), Arc::clone(&input)).unwrap();
        let continued = target.continued_synth(&source).unwrap();
        target.preserve_from(&mut source, continued);
        let mut output = [[0.0; 2]; 128];
        target.mix(&mut output);
        assert_eq!(input.held_keys(), [0, 1 << 63]);
        assert!(output.iter().any(|frame| frame[0] != 0.0));
        input.set_key(0, None).unwrap();
        target.mix(&mut output);
        assert_eq!(input.held_keys(), [0; 2]);
    }
}
