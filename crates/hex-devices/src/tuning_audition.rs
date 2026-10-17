//! Fixed-capacity reference-tone synthesizer for project-tuning audition.
//!
//! The oscillator shapes, PolyBLEP correction, envelope laws, and output
//! saturation are adapted from the user's local Freqs reference
//! at `/Users/portalsurfer/dev/freqs/src/synth.rs` (triangle/square/saw,
//! 16 voices, `exp(-4.6 / attack_samples)`, `exp(-9.2 / release_samples)`,
//! and `(sum * level * 0.65).tanh()`). This module has no runtime dependency
//! on Freqs and performs no allocation, locking, I/O, or coefficient preparation
//! while rendering samples. The reference output tanh saturation remains per-sample.

use core::fmt;

/// Maximum number of simultaneous audition voices.
pub const MAX_VOICES: usize = 16;
const KEY_COUNT: usize = 128;
const RELEASE_SILENCE: f32 = 0.0001;

/// Oscillator shape for a reference tone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waveform {
    Triangle,
    Square,
    Saw,
}

/// Validation and ownership errors returned by the allocation-free kernel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditionError {
    InvalidSampleRate,
    InvalidFrequency,
    InvalidLevel,
    InvalidAttack,
    InvalidRelease,
    InvalidKey,
    UnsupportedKey,
    InvalidVelocity,
    InvalidOwner,
    SampleRateMismatch,
    AgeExhausted,
}

impl fmt::Display for AuditionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSampleRate => "invalid sample rate",
            Self::InvalidFrequency => "invalid tuning frequency",
            Self::InvalidLevel => "invalid audition level",
            Self::InvalidAttack => "invalid audition attack",
            Self::InvalidRelease => "invalid audition release",
            Self::InvalidKey => "key is outside the midi range",
            Self::UnsupportedKey => "key is at or above the nyquist limit",
            Self::InvalidVelocity => "velocity must be finite and in (0, 1]",
            Self::InvalidOwner => "owner token must be nonzero",
            Self::SampleRateMismatch => "retuning requires the same sample rate",
            Self::AgeExhausted => "audition voice age exhausted",
        })
    }
}

/// Immutable, control-side settings and lookup tables for a `Synth`.
///
/// Frequencies at or above Nyquist are explicitly marked unsupported. Their
/// phase steps are zero and `Synth::note_on` refuses them rather than folding
/// or clamping them into the audible band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreparedAudition {
    sample_rate: u32,
    supported: [bool; KEY_COUNT],
    phase_steps: [f64; KEY_COUNT],
    waveform: Waveform,
    level: f32,
    attack_coefficient: f32,
    release_coefficient: f32,
}

impl PreparedAudition {
    /// Validate settings and prepare per-key phase increments and envelope
    /// coefficients. This constructor belongs on the control side.
    pub fn new(
        sample_rate: u32,
        frequencies: [f64; KEY_COUNT],
        waveform: Waveform,
        level: f32,
        attack_ms: f32,
        release_ms: f32,
    ) -> Result<Self, AuditionError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(AuditionError::InvalidSampleRate);
        }
        if !level.is_finite() || !(0.0..=1.0).contains(&level) {
            return Err(AuditionError::InvalidLevel);
        }
        if !attack_ms.is_finite() || !(2.0..=500.0).contains(&attack_ms) {
            return Err(AuditionError::InvalidAttack);
        }
        if !release_ms.is_finite() || !(100.0..=3_000.0).contains(&release_ms) {
            return Err(AuditionError::InvalidRelease);
        }

        let nyquist = f64::from(sample_rate) * 0.5;
        let mut supported = [false; KEY_COUNT];
        let mut phase_steps = [0.0; KEY_COUNT];
        for (index, frequency) in frequencies.into_iter().enumerate() {
            if !frequency.is_finite() || frequency <= 0.0 {
                return Err(AuditionError::InvalidFrequency);
            }
            let step = frequency / f64::from(sample_rate);
            if frequency < nyquist && step > 0.0 {
                supported[index] = true;
                phase_steps[index] = step;
            }
        }

        let attack_samples = f64::from(attack_ms) * f64::from(sample_rate) * 0.001;
        let release_samples = f64::from(release_ms) * f64::from(sample_rate) * 0.001;
        Ok(Self {
            sample_rate,
            supported,
            phase_steps,
            waveform,
            level,
            attack_coefficient: (-4.6 / attack_samples).exp() as f32,
            release_coefficient: (-9.2 / release_samples).exp() as f32,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Return whether a MIDI key has a positive, strictly sub-Nyquist phase
    /// increment in this prepared table.
    pub fn supports_key(&self, key: u8) -> bool {
        let index = usize::from(key);
        index < KEY_COUNT && self.supported[index]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Voice {
    active: bool,
    held: bool,
    key: u8,
    owner: u64,
    age: u64,
    phase: f64,
    velocity: f32,
    envelope: f32,
}

/// Complete fixed-storage synth state. It is `Copy` so a control-side owner
/// can prepare a twin and publish/rollback it atomically with its prepared
/// playback candidate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Synth {
    prepared: PreparedAudition,
    voices: [Voice; MAX_VOICES],
    next_age: u64,
}

impl Synth {
    pub fn new(prepared: PreparedAudition) -> Self {
        Self {
            prepared,
            voices: [Voice::default(); MAX_VOICES],
            next_age: 1,
        }
    }

    /// Start a note owned by a nonzero caller token. Tokens identify the
    /// lifetime of the note source; callers must use a fresh token when an old
    /// note-off could still arrive for a previous lifetime.
    pub fn note_on(&mut self, key: u8, velocity: f32, owner: u64) -> Result<(), AuditionError> {
        let index = usize::from(key);
        if index >= KEY_COUNT {
            return Err(AuditionError::InvalidKey);
        }
        if !velocity.is_finite() || !(0.0..=1.0).contains(&velocity) || velocity == 0.0 {
            return Err(AuditionError::InvalidVelocity);
        }
        if owner == 0 {
            return Err(AuditionError::InvalidOwner);
        }
        if !self.prepared.supported[index] {
            return Err(AuditionError::UnsupportedKey);
        }

        if self.next_age == u64::MAX {
            return Err(AuditionError::AgeExhausted);
        }
        let voice_index = self
            .voices
            .iter()
            .position(|voice| !voice.active)
            .or_else(|| oldest_voice(&self.voices, false))
            .or_else(|| oldest_voice(&self.voices, true))
            .unwrap_or(0);
        self.voices[voice_index] = Voice {
            active: true,
            held: true,
            key,
            owner,
            age: self.next_age,
            phase: 0.0,
            velocity,
            envelope: 0.0,
        };
        self.next_age += 1;
        Ok(())
    }

    /// Release every held voice with exactly this owner token. A stale token
    /// cannot release a stolen/replacement voice owned by another token.
    pub fn note_off(&mut self, owner: u64) {
        if owner == 0 {
            return;
        }
        for voice in &mut self.voices {
            if voice.active && voice.held && voice.owner == owner {
                voice.held = false;
            }
        }
    }

    /// Begin release envelopes for every active voice.
    pub fn release_all(&mut self) {
        for voice in &mut self.voices {
            voice.held = false;
        }
    }

    /// Immediately silence all voices.
    pub fn clear(&mut self) {
        self.voices = [Voice::default(); MAX_VOICES];
        self.next_age = 1;
    }

    /// Retarget all current voices to new tuning/settings without changing
    /// phase, envelope, ownership, held state, age, or velocity. Every active
    /// key and the sample rate are preflighted before `self` is modified.
    pub fn retarget_from(
        &mut self,
        source: &Self,
        target: PreparedAudition,
    ) -> Result<(), AuditionError> {
        if source.prepared.sample_rate != target.sample_rate {
            return Err(AuditionError::SampleRateMismatch);
        }
        for voice in source.voices.iter().filter(|voice| voice.active) {
            let index = usize::from(voice.key);
            if index >= KEY_COUNT || !target.supported[index] {
                return Err(AuditionError::UnsupportedKey);
            }
        }
        *self = *source;
        self.prepared = target;
        Ok(())
    }

    /// Render one bounded mono sample. All rate-dependent transcendental
    /// values and key phase increments were prepared before this call.
    pub fn next_sample(&mut self) -> f32 {
        let mut sum = 0.0_f32;
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            let index = usize::from(voice.key);
            if voice.held {
                voice.envelope = 1.0 - (1.0 - voice.envelope) * self.prepared.attack_coefficient;
            } else {
                voice.envelope *= self.prepared.release_coefficient;
                if voice.envelope < RELEASE_SILENCE {
                    *voice = Voice::default();
                    continue;
                }
            }

            let step = self.prepared.phase_steps[index];
            let oscillator = match self.prepared.waveform {
                Waveform::Triangle => 1.0 - 4.0 * (voice.phase - 0.5).abs(),
                Waveform::Square => {
                    let raw = if voice.phase < 0.5 { 1.0 } else { -1.0 };
                    raw + poly_blep(voice.phase, step)
                        - poly_blep(wrap_phase(voice.phase + 0.5), step)
                }
                Waveform::Saw => 2.0 * voice.phase - 1.0 - poly_blep(voice.phase, step),
            };
            sum += oscillator as f32 * voice.envelope * voice.velocity;
            voice.phase = wrap_phase(voice.phase + step);
        }
        (sum * self.prepared.level * 0.65).tanh()
    }

    pub fn held_key_mask(&self) -> [u64; 2] {
        let mut keys = [0_u64; 2];
        for voice in self
            .voices
            .iter()
            .filter(|voice| voice.active && voice.held)
        {
            let key = usize::from(voice.key);
            keys[key / 64] |= 1_u64 << (key % 64);
        }
        keys
    }

    /// Bounded owner lookup for preserving physical inputs across a retune.
    pub fn owns_voice(&self, owner: u64) -> bool {
        self.voices
            .iter()
            .any(|voice| voice.active && voice.owner == owner)
    }

    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|voice| voice.active).count()
    }
}

fn oldest_voice(voices: &[Voice; MAX_VOICES], held: bool) -> Option<usize> {
    voices
        .iter()
        .enumerate()
        .filter(|(_, voice)| voice.active && voice.held == held)
        .min_by_key(|(_, voice)| voice.age)
        .map(|(index, _)| index)
}

fn wrap_phase(phase: f64) -> f64 {
    if phase >= 1.0 { phase - 1.0 } else { phase }
}

fn poly_blep(phase: f64, step: f64) -> f64 {
    if phase < step {
        let t = phase / step;
        t + t - t * t - 1.0
    } else if phase > 1.0 - step {
        let t = (phase - 1.0) / step;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

impl std::error::Error for AuditionError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn frequencies(rate: u32) -> [f64; KEY_COUNT] {
        let mut values = [0.0; KEY_COUNT];
        for (key, value) in values.iter_mut().enumerate() {
            *value = 55.0 * 2.0_f64.powf((key as f64 - 33.0) / 12.0);
        }
        values[127] = f64::from(rate) * 0.5;
        values
    }

    fn prepared(rate: u32, frequencies: [f64; KEY_COUNT], waveform: Waveform) -> PreparedAudition {
        PreparedAudition::new(rate, frequencies, waveform, 0.6, 10.0, 900.0).unwrap()
    }

    #[test]
    fn all_waveforms_are_finite_bounded_and_have_attack_release_tails() {
        for waveform in [Waveform::Triangle, Waveform::Square, Waveform::Saw] {
            let mut synth = Synth::new(prepared(48_000, frequencies(48_000), waveform));
            synth.note_on(60, 0.8, 7).unwrap();
            let peak = (0..24_000)
                .map(|_| synth.next_sample().abs())
                .fold(0.0_f32, f32::max);
            assert!(peak > 0.01);
            synth.note_off(7);
            let early_tail = (0..2_400)
                .map(|_| synth.next_sample().abs())
                .fold(0.0_f32, f32::max);
            assert!(early_tail > 0.001);
            for _ in 0..60_000 {
                let sample = synth.next_sample();
                assert!(sample.is_finite() && sample.abs() <= 1.0);
            }
            assert_eq!(synth.active_voices(), 0);
        }
    }

    #[test]
    fn note_off_is_owner_exact_and_voice_stealing_is_oldest_released_first() {
        let mut synth = Synth::new(prepared(48_000, frequencies(48_000), Waveform::Saw));
        for key in 40..56 {
            synth.note_on(key, 0.5, u64::from(key) + 1).unwrap();
        }
        synth.note_off(41); // release key 40
        synth.note_on(70, 0.5, 999).unwrap();
        assert_eq!(synth.voices[0].key, 70);
        assert!(synth.voices[0].held);
        synth.note_off(41); // a late duplicate cannot release the replacement
        assert!(synth.voices[0].held);

        // With no released voice, deterministic oldest-held stealing uses the
        // next-oldest voice (slot 1; slot 0 contains the newer replacement).
        synth.note_on(71, 0.5, 1_000).unwrap();
        assert_eq!(synth.voices[1].key, 71);
        assert_eq!(synth.active_voices(), MAX_VOICES);
    }

    #[test]
    fn malformed_inputs_and_unsupported_keys_refuse_without_state_change() {
        let table = frequencies(48_000);
        assert_eq!(
            PreparedAudition::new(7_999, table, Waveform::Triangle, 0.5, 10.0, 900.0),
            Err(AuditionError::InvalidSampleRate)
        );
        let mut bad = table;
        bad[2] = f64::NAN;
        assert_eq!(
            PreparedAudition::new(48_000, bad, Waveform::Triangle, 0.5, 10.0, 900.0),
            Err(AuditionError::InvalidFrequency)
        );
        let mut synth = Synth::new(prepared(48_000, table, Waveform::Triangle));
        let before = synth;
        assert_eq!(synth.note_on(128, 0.5, 1), Err(AuditionError::InvalidKey));
        assert_eq!(
            synth.note_on(60, f32::INFINITY, 1),
            Err(AuditionError::InvalidVelocity)
        );
        assert_eq!(synth.note_on(60, 0.5, 0), Err(AuditionError::InvalidOwner));
        assert_eq!(
            synth.note_on(127, 0.5, 1),
            Err(AuditionError::UnsupportedKey)
        );
        assert_eq!(synth, before);
        assert!(!synth.prepared.supports_key(127));
        assert!(synth.prepared.phase_steps[127] == 0.0);
    }

    #[test]
    fn retarget_preserves_complete_voice_state_or_refuses_atomically() {
        let old_frequencies = frequencies(48_000);
        let mut source = Synth::new(prepared(48_000, old_frequencies, Waveform::Triangle));
        source.note_on(60, 0.7, 22).unwrap();
        for _ in 0..1_500 {
            source.next_sample();
        }
        source.note_on(64, 0.4, 23).unwrap();
        for _ in 0..400 {
            source.next_sample();
        }
        source.note_off(23);
        for _ in 0..300 {
            source.next_sample();
        }
        assert!(
            source
                .voices
                .iter()
                .any(|voice| voice.active && voice.key == 64)
        );

        let mut target = Synth::new(prepared(48_000, old_frequencies, Waveform::Square));
        let old_target = target;
        let mut unsupported = old_frequencies;
        unsupported[64] = 24_000.0;
        assert_eq!(
            target.retarget_from(&source, prepared(48_000, unsupported, Waveform::Saw)),
            Err(AuditionError::UnsupportedKey)
        );
        assert_eq!(target, old_target);

        let mut retuned = old_frequencies;
        retuned[60] *= 1.5;
        retuned[64] *= 1.25;
        target
            .retarget_from(&source, prepared(48_000, retuned, Waveform::Saw))
            .unwrap();
        assert_eq!(target.voices, source.voices);
        assert_eq!(target.prepared.waveform, Waveform::Saw);
        assert_eq!(target.prepared.phase_steps[60], retuned[60] / 48_000.0);

        let wrong_rate = prepared(44_100, frequencies(44_100), Waveform::Square);
        let before = target;
        assert_eq!(
            target.retarget_from(&source, wrong_rate),
            Err(AuditionError::SampleRateMismatch)
        );
        assert_eq!(target, before);
    }
}
