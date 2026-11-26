//! Fully owned custom spectra and independent transactional voice histories.
//! The current variable-size instrument contract charges the entire twin.
use super::*;
use crate::operator_waveform::{OperatorWaveformBank, harmonic_cap_index, prepared_bank};
use hex_lattice::{
    DeviceResources, InstrumentHistoryRetargetPreparation, InstrumentParameterRuntime,
    MAX_INSTRUMENT_AUTOMATION_FRAMES, MutableInstrumentRuntime, NativeInstrumentClock,
    NativeInstrumentEvent, NativeInstrumentOccurrenceGain, NativeInstrumentRuntimeError as Error,
    NativeParameterAutomation, PreparedInstrumentHistoryRetarget, PreparedMutableInstrumentRuntime,
};
use hex_lattice_contract::{ParameterDescriptor, ParameterId, ParameterSemantics, ParameterValue};
use std::any::Any;

mod filter;
mod modulation;
mod quality;
mod voices;
use voices::Ledger;
mod spectrum;
pub(super) use filter::Trajectory as FilterTrajectory;
pub(super) use spectrum::Spectrum;
use spectrum::{HarmonicPhaseTable, harmonic_phase_table};
const VOICES: usize = 64;

#[derive(Clone)]
pub(super) struct Factory {
    pub configuration: PreparedOperatorConfiguration,
    // Kept out of the small legacy configuration, which callback parameter
    // interpolation copies. This fixed owner is charged by the wide runtime.
    pub tuning: Option<hex_lattice::TuningTable>,
    pub base: [u8; ENABLE_PAYLOAD_BYTES],
    pub settings: OperatorSettings,
    target: Spectrum,
    table: &'static SineTable,
    bank: &'static OperatorWaveformBank,
    phase_table: &'static HarmonicPhaseTable,
    lfo_table: &'static quality::LfoTable,
    rate: u32,
    parameters: [f64; 12],
    filter: FilterTrajectory,
    companion_active: bool,
    // Identical authored machine laws start equal and receive the same
    // per-voice age, clock observations and note-off. Active history may only
    // transfer across compatible companion and envelope laws. Per-operator
    // machine events would require invalidating this proof.
    uniform_companion: bool,
    companion_sources: [u8; 4],
    partial_companion: bool,
    shared_loop_timing: bool,
    uniform_envelopes: bool,
}
impl Factory {
    #[cfg(test)]
    pub fn new(
        settings: OperatorSettings,
        configuration: PreparedOperatorConfiguration,
        table: &'static SineTable,
        rate: u32,
    ) -> Result<Self, DeviceContractError> {
        Self::new_with_tuning(settings, configuration, table, rate, None)
    }
    pub fn new_with_tuning(
        settings: OperatorSettings,
        configuration: PreparedOperatorConfiguration,
        table: &'static SineTable,
        rate: u32,
        tuning: Option<hex_lattice::TuningTable>,
    ) -> Result<Self, DeviceContractError> {
        let _state = settings.to_state()?;
        let base = settings.encode_base_prefix();
        let parameters = parameter_words(
            configuration
                .feedback_parameters(settings.effective_feedback().map(f64::from))
                .map_err(|_| DeviceContractError::InvalidParameterValue)?,
        );
        // Each alias points backward to the first bit-identical machine law.
        let mut companion_sources = [0, 1, 2, 3];
        for (operator, source) in companion_sources.iter_mut().enumerate().skip(1) {
            for previous in 0..operator {
                if settings.companion.words[operator] == settings.companion.words[previous]
                    && (0..ENVELOPE_FIELDS).all(|field| {
                        settings.fields[CORE_FIELDS + operator * ENVELOPE_FIELDS + field].to_bits()
                            == settings.fields[CORE_FIELDS + previous * ENVELOPE_FIELDS + field]
                                .to_bits()
                    })
                {
                    *source = previous as u8;
                    break;
                }
            }
        }
        let uniform_companion = companion_sources == [0, 0, 0, 0];
        let partial_companion = !uniform_companion && companion_sources != [0, 1, 2, 3];
        // Matching mode, velocity time scaling, loop return, attack and decay
        // imply equal prepared timing for every note velocity at this rate.
        let shared_loop_timing = !uniform_companion
            && !partial_companion
            && (0..4).all(|i| {
                settings.companion.words[i][5] == 1
                    && [3, 6]
                        .into_iter()
                        .all(|j| settings.companion.words[i][j] == settings.companion.words[0][j])
                    && [3, 4].into_iter().all(|j| {
                        settings.fields[CORE_FIELDS + i * ENVELOPE_FIELDS + j].to_bits()
                            == settings.fields[CORE_FIELDS + j].to_bits()
                    })
            });
        Ok(Self {
            uniform_envelopes: configuration.has_uniform_envelopes(),
            configuration,
            tuning,
            base,
            target: Spectrum::prepare(&settings),
            filter: if settings.global.active {
                FilterTrajectory::oversampled(settings.filter, rate)?
            } else {
                FilterTrajectory::new(settings.filter, rate)?
            },
            companion_active: settings.companion.requires_runtime(),
            uniform_companion,
            companion_sources,
            partial_companion,
            shared_loop_timing,
            settings,
            table,
            phase_table: harmonic_phase_table()?,
            lfo_table: quality::lfo_table(),
            bank: prepared_bank().map_err(|_| DeviceContractError::RuntimeAllocation)?,
            rate,
            parameters,
        })
    }
    fn prepare_voice(
        &self,
        pitch: u8,
        velocity: u8,
        origin: u64,
        beat: Option<u64>,
        running: Option<bool>,
    ) -> Result<Voice, Error> {
        let note = if self.settings.lfo.active() {
            self.configuration
                .prepare_lfo_note_with_tuning(pitch, velocity, self.tuning.as_ref())
        } else {
            self.configuration
                .prepare_note_with_tuning(pitch, velocity, self.tuning.as_ref())
        }
        .map_err(|_| Error::InvalidEventBatch)?;
        let settings = self.settings;
        let machines = if settings.companion.requires_runtime() {
            let envelopes = settings
                .companion
                .envelopes(&settings.fields, velocity)
                .map_err(|_| Error::InvalidEventBatch)?;
            let machines = std::array::from_fn(|i| {
                let base = CORE_FIELDS + i * ENVELOPE_FIELDS;
                companion::Machine::prepare(
                    settings.companion,
                    i,
                    envelopes[i],
                    std::array::from_fn(|j| f64::from(settings.fields[base + j])),
                    self.rate,
                    beat,
                    running,
                )
                .map(Some)
            });
            let [a, b, c, d] = machines;
            [a?, b?, c?, d?]
        } else {
            [None; 4]
        };
        let phases = std::array::from_fn(|i| {
            if settings.companion.words[i][0] == 0 {
                PreparedOperatorConfiguration::lfo_steps(&note)[i].wrapping_mul(origin)
            } else {
                0
            }
        });
        let pitch_env = if settings.pitch_env.enabled() {
            Some(mod_env::Machine::prepare(
                settings.pitch_env.spec(),
                self.rate,
                beat,
                running,
            )?)
        } else {
            None
        };
        let filter_env = if settings.filter.armed && settings.filter_env.amount() != 0.0 {
            Some(mod_env::Machine::prepare(
                settings.filter_env.spec(),
                self.rate,
                beat,
                running,
            )?)
        } else {
            None
        };
        Ok(Voice {
            note,
            phases,
            machines,
            pitch_env,
            filter_env,
            ..Voice::default()
        })
    }
    fn capacity(&self) -> usize {
        if self.settings.global.active {
            return 64;
        }
        if self.settings.lfo.active() || self.settings.effective_feedback() != [0.0; 4] {
            16
        } else {
            64
        }
    }
    fn retuned_note(&self, source: PreparedOperatorNote) -> Result<PreparedOperatorNote, Error> {
        let (pitch, velocity) = source.midi_origin();
        if self.settings.lfo.active() {
            self.configuration
                .prepare_lfo_note_with_tuning(pitch, velocity, self.tuning.as_ref())
        } else {
            self.configuration
                .prepare_note_with_tuning(pitch, velocity, self.tuning.as_ref())
        }
        .map_err(|_| Error::InvalidEventBatch)
    }
    fn compatible(&self, other: &Self) -> bool {
        self.rate == other.rate
            && self.settings.companion == other.settings.companion
            && compatible_base(&self.base, &other.base, self.settings.global.active)
    }
    pub(super) fn effective_time_rate(&self, voice_time_rate: f64) -> f64 {
        let global_time = self.settings.performance.time_rate();
        if global_time == 1.0 {
            voice_time_rate
        } else {
            (voice_time_rate * global_time).clamp(0.25, 16.0)
        }
    }

    fn parameters(&self) -> [f64; 12] {
        self.parameters
    }
}
fn compatible_base(
    a: &[u8; ENABLE_PAYLOAD_BYTES],
    b: &[u8; ENABLE_PAYLOAD_BYTES],
    allow_routing: bool,
) -> bool {
    // An unrelated graph edit commonly inherits an unchanged instrument.
    // Compare its prepared bytes in bulk before checking retargetable fields.
    if a == b {
        return true;
    }
    a.iter().zip(b).enumerate().all(|(i, (a, b))| {
        let levels = [4..8, 16..20, 28..32, 40..44]
            .iter()
            .any(|r| r.contains(&i));
        let carriers = (112..128).contains(&i);
        let waveforms = (LEGACY_PAYLOAD_BYTES..WAVEFORM_PAYLOAD_BYTES).contains(&i);
        let feedback = (VELOCITY_PAYLOAD_BYTES..FEEDBACK_PAYLOAD_BYTES).contains(&i);
        let enabled = (LFO_PAYLOAD_BYTES..ENABLE_PAYLOAD_BYTES).contains(&i);
        let lfo = (FEEDBACK_PAYLOAD_BYTES..LFO_PAYLOAD_BYTES).contains(&i);
        let routing = allow_routing && (48..112).contains(&i);
        // Keytrack's fixed endpoint mirrors the legacy fixed-mode word. The
        // target prepares all new phase steps before preserving voice history.
        let pitch_mode = (WAVEFORM_PAYLOAD_BYTES..TUNING_PAYLOAD_BYTES).contains(&i)
            && (i - WAVEFORM_PAYLOAD_BYTES) % 12 < 4;
        routing
            || levels
            || carriers
            || waveforms
            || feedback
            || enabled
            || lfo
            || pitch_mode
            || a == b
    })
}
// Keep the partially shared path out of the distinct-envelope render path.
#[inline(never)]
#[cold]
fn partial_levels(
    machines: &[Option<companion::Machine>; 4],
    sources: [u8; 4],
    age: u64,
) -> [f64; 4] {
    let [a, b, c, d] = machines;
    let a = a.as_ref().map_or(0.0, |machine| machine.level(age));
    let b = if sources[1] == 0 {
        a
    } else {
        b.as_ref().map_or(0.0, |machine| machine.level(age))
    };
    let c = match sources[2] {
        0 => a,
        1 => b,
        _ => c.as_ref().map_or(0.0, |machine| machine.level(age)),
    };
    let d = match sources[3] {
        0 => a,
        1 => b,
        2 => c,
        _ => d.as_ref().map_or(0.0, |machine| machine.level(age)),
    };
    [a, b, c, d]
}
#[derive(Clone, Copy, Default)]
pub(super) struct Voice {
    pub note: PreparedOperatorNote,
    pub phases: [u64; 4],
    pub previous: [f64; 4],
    pub age: u64,
    pub gate: Option<u64>,
    pub id: u64,
    pub sequence: u64,
    pub decimator: quality::Decimator,
    pub effective: super::modulation::Effective,
    pub modulated_filter: Option<FilterTrajectory>,
    pub envelope_age: f64,
    pub envelope_gate: Option<u64>,
    pub drain: u32,
    pub pitch_steps: [u64; 4],
    pub pan_gains: [f32; 2],
    pub lfo_rate_multiplier: f64,
    pub filters: [crate::operator_filter::Carrier; 4],
    pub lfo_fraction: f64,
    pub lfo_cycle: u64,
    pub machines: [Option<companion::Machine>; 4],
    pub pitch_offset_cents: i16,
    pub timbre: u8,
    pub glide_semitones: f64,
    pub glide_target_pitch: f64,
    pub pitch_env: Option<mod_env::Machine>,
    pub filter_env: Option<mod_env::Machine>,
}
#[derive(Clone, Copy)]
pub(super) struct ParameterRamp {
    pub source: [f64; 12],
    pub elapsed: u32,
    pub frames: u32,
}
impl ParameterRamp {
    fn at(self, target: [f64; 12]) -> [f64; 12] {
        let amount = f64::from(self.elapsed.min(self.frames)) / f64::from(self.frames);
        std::array::from_fn(|i| {
            if amount == 1.0 {
                target[i]
            } else {
                self.source[i] + (target[i] - self.source[i]) * amount
            }
        })
    }
}
/// Fixed inline pool for stolen voices undergoing a ~1 ms linear de-click fade.
pub(super) const STEAL_FADE_SLOTS: usize = 8;

/// Stolen voice fading linearly to silence over ~1 ms so polyphony saturation
/// does not introduce an audible step discontinuity.
#[derive(Clone, Copy, Default)]
pub(super) struct StealFade {
    pub voice: Voice,
    pub remaining: u16,
    pub total: u16,
}

pub(super) fn steal_fade_frames(rate: u32) -> u16 {
    hex_dsp::ramp_frames_for_ms(rate, 1.0)
        .unwrap_or(1)
        .clamp(1, u32::from(u16::MAX)) as u16
}

pub(super) struct Runtime {
    factory: Factory,
    voices: [Voice; VOICES],
    fading: [StealFade; STEAL_FADE_SLOTS],
    source: Spectrum,
    elapsed: u32,
    frames: u32,
    parameters: Option<ParameterRamp>,
    filter: FilterTrajectory,
    phase: (f64, u64),
    local_frame: u64,
    ledger: Ledger,
    expression: [f32; 3],
    channel_pitch_bend: i16,
    cc_timbre: u8,
    last_targets: [f32; automation::TARGET_COUNT],
    tone_filter: performance::ToneFilter,
    last_note_pitch: Option<f64>,
    spread_stage: pitch_perf::SpreadStage,
}

// Borrow the source bank through validation/retargeting. Legacy runtimes convert
// one voice at a time, so no adapter materializes a 64-machine-bank temporary.
pub(super) trait VoiceHistory {
    fn voice(&self, index: usize) -> Option<Voice>;
    fn admission_voice(&self, index: usize) -> Option<(u64, PreparedOperatorNote)> {
        self.voice(index).map(|voice| (voice.id, voice.note))
    }
}

mod events;
mod legacy;
mod render;
mod runtime_impl;
use events::{EventProof, EventVoice};
use legacy::Admission;
pub(super) use legacy::{Legacy, builtin_spectrum, parameter_words};

#[derive(Clone, Copy)]
struct Target(u64);
impl PreparedInstrumentHistoryRetarget for Target {
    fn allows_runtime_versions(&self, source: u32, target: u32) -> bool {
        target == 3 && matches!(source, 1..=3)
    }
    fn declared_fixed_storage_bytes(&self) -> usize {
        size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl PreparedMutableInstrumentRuntime for Factory {
    fn parameter_automation_descriptors(&self) -> &[ParameterDescriptor] {
        &automation::TARGETS
    }
    fn parameter_modulation_semantics(&self, parameter: ParameterId) -> Option<ParameterSemantics> {
        automation::parameter_semantics(parameter)
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn can_import_analytic_context(
        &self,
        source: &dyn hex_lattice::PreparedSharedAnalyticContext,
    ) -> bool {
        self.capacity() == VOICES
            && self.filter.armed
            && !self.settings.lfo.active()
            && self.settings.effective_feedback() == [0.0; OPERATORS]
            && source
                .import_identity()
                .and_then(|p| p.downcast_ref::<PreparedOperatorContext>())
                .is_some_and(|p| {
                    self.configuration.identical_to(&p.configuration)
                        && std::ptr::eq(self.table, p.sine_table)
                })
    }
    fn runtime_version(&self) -> u32 {
        3
    }
    fn declared_twin_bytes(&self) -> usize {
        size_of::<Runtime>()
    }
    fn max_events_per_block(&self) -> usize {
        if self.settings.global.active {
            259
        } else {
            256
        }
    }
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn max_active_occurrences(&self) -> usize {
        // Stolen held notes can resume in mono mode; their identities remain
        // live independently of the audible voice count.
        if self.settings.global.active {
            VOICES
        } else {
            self.capacity()
        }
    }
    fn max_release_tail_frames(&self) -> u64 {
        if self.settings.companion.requires_runtime() {
            140 * u64::from(self.rate)
        } else {
            self.configuration.release_frames()
        }
        .saturating_mul(
            if self.settings.global.active || self.settings.performance.time > 0.0 {
                4
            } else {
                1
            },
        )
        .saturating_add(if self.settings.global.active {
            quality::TAIL
        } else {
            0
        })
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(Runtime {
            factory: self.clone(),
            voices: [Voice::default(); VOICES],
            fading: [StealFade::default(); STEAL_FADE_SLOTS],
            source: self.target,
            elapsed: 0,
            frames: 1,
            parameters: None,
            filter: self.filter,
            phase: (0.0, 0),
            local_frame: 0,
            ledger: Ledger::default(),
            expression: [0.0; 3],
            channel_pitch_bend: 0,
            cc_timbre: 0,
            last_targets: automation::base_target_values(&self.settings),
            tone_filter: performance::ToneFilter::default(),
            last_note_pitch: None,
            spread_stage: pitch_perf::SpreadStage::default(),
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn prepare_history_retarget_target(
        &self,
    ) -> Result<InstrumentHistoryRetargetPreparation, DeviceContractError> {
        Ok(InstrumentHistoryRetargetPreparation::Prepared(Box::new(
            Target(self.configuration.configuration_id()),
        )))
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod global_tests;

#[cfg(test)]
mod fine_tests;
