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
    pub fn rejected(&self) -> bool {
        self.rejected.load(Acquire)
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
    pub(crate) fn continued_synth(&self, old: &Self) -> Result<Synth, Invalid> {
        let mut scratch = self.synth;
        scratch
            .retarget_from(&old.synth, self.prepared)
            .map_err(|_| Invalid("held audition pitch cannot be retuned"))?;
        Ok(scratch)
    }
    pub(crate) fn preserve_from(&mut self, old: &mut Self, continued: Synth) {
        self.synth = continued;
        self.desired = old.desired;
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
                if self.synth.note_on(key, velocity, next >> 16).is_err() {
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
