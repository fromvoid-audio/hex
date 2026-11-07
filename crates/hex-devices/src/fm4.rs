//! Original standalone phase-modulation kernel, a prerequisite for Operator.
//!
//! The built-in definition admits this bounded acyclic kernel to native MIDI
//! playback. The legacy FM4 device remains memoryless; Operator's separate
//! standalone feedback voice adds bounded sequential self-feedback without
//! changing native admission. State and note preparation validate on control;
//! sampling uses fixed arrays and borrowed tables.
//! Graph coefficients are modulation depth in turns.
pub mod device;
mod operator;
pub(crate) use operator::{FINE_LIMIT_HZ, OperatorFeedbackRetargetFrame};
pub use operator::{
    OperatorFeedbackParameters, OperatorTuning, PreparedOperatorConfiguration,
    PreparedOperatorFeedbackRetarget, PreparedOperatorFeedbackVoice, PreparedOperatorFm4,
    PreparedOperatorNote,
};

use crate::operator_waveform::{OperatorWaveform, OperatorWaveformBank};
use crate::{Invalid, PHASE_SCALE, SineTable};
use hex_lattice::{ANALYTIC_NOTE_PAYLOAD_WORDS, PreparedAnalyticNote, StereoFrame};

pub const OPERATORS: usize = 4;

/// Immutable oscillator settings. Level affects both modulation and carriers.
#[derive(Debug, Clone, Copy)]
pub struct Oscillator {
    ratio: f64,
    level: f64,
    phase_turns: f64,
}

impl Oscillator {
    /// Original bounded domains, not measured Live parameter transfer laws.
    pub fn new(ratio: f64, level: f64, phase_turns: f64) -> Result<Self, Invalid> {
        if !ratio.is_finite() || !(1.0 / 128.0..=128.0).contains(&ratio) {
            return Err(Invalid("fm ratio must be finite and between 1/128 and 128"));
        }
        if !level.is_finite() || !(0.0..=8.0).contains(&level) {
            return Err(Invalid("fm level must be finite and between 0 and 8"));
        }
        if !phase_turns.is_finite() || !(0.0..1.0).contains(&phase_turns) {
            return Err(Invalid(
                "fm initial phase must be finite and in [0, 1) turns",
            ));
        }
        Ok(Self {
            ratio,
            level,
            phase_turns,
        })
    }
}

/// Validated directed graph: `modulation[source][target]`, plus output weights.
/// Cross-modulation edges form a DAG and may use any oscillator order. The
/// separate Operator feedback voice handles self-feedback outside this matrix.
#[derive(Debug, Clone, Copy)]
pub struct Routing {
    modulation: [[f64; OPERATORS]; OPERATORS],
    carriers: [f64; OPERATORS],
    order: [usize; OPERATORS],
}

impl Routing {
    pub fn new(
        modulation: [[f64; OPERATORS]; OPERATORS],
        carriers: [f64; OPERATORS],
    ) -> Result<Self, Invalid> {
        if modulation
            .iter()
            .flatten()
            .any(|depth| !depth.is_finite() || !(-8.0..=8.0).contains(depth))
            || carriers
                .iter()
                .any(|weight| !weight.is_finite() || !(-1.0..=1.0).contains(weight))
        {
            return Err(Invalid(
                "fm routing coefficients are outside their finite domain",
            ));
        }
        let mut order = [0; OPERATORS];
        let mut visited = [false; OPERATORS];
        for slot in &mut order {
            let Some(target) = (0..OPERATORS).find(|&target| {
                !visited[target]
                    && (0..OPERATORS)
                        .all(|source| modulation[source][target] == 0.0 || visited[source])
            }) else {
                return Err(Invalid(
                    "fm routing must be acyclic; feedback requires a voice runtime",
                ));
            };
            visited[target] = true;
            *slot = target;
        }
        Ok(Self {
            modulation,
            carriers,
            order,
        })
    }
}

/// Original linear ADSR policy. Stage durations are seconds, bounded to 120.
#[derive(Debug, Clone, Copy)]
pub struct Envelope {
    attack_seconds: f64,
    decay_seconds: f64,
    sustain: f64,
    release_seconds: f64,
}

impl Envelope {
    pub fn new(
        attack_seconds: f64,
        decay_seconds: f64,
        sustain: f64,
        release_seconds: f64,
    ) -> Result<Self, Invalid> {
        if [attack_seconds, decay_seconds, release_seconds]
            .iter()
            .any(|time| !time.is_finite() || !(0.0..=120.0).contains(time))
            || !sustain.is_finite()
            || !(0.0..=1.0).contains(&sustain)
        {
            return Err(Invalid(
                "fm envelope needs finite stage times [0, 120] and sustain [0, 1]",
            ));
        }
        Ok(Self {
            attack_seconds,
            decay_seconds,
            sustain,
            release_seconds,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct PreparedEnvelope {
    attack: u64,
    decay: u64,
    sustain: f64,
    release: u64,
}

impl PreparedEnvelope {
    fn held_level(self, age: u64) -> f64 {
        if age < self.attack {
            age as f64 / self.attack as f64
        } else if age - self.attack < self.decay {
            1.0 - (age - self.attack) as f64 / self.decay as f64 * (1.0 - self.sustain)
        } else {
            self.sustain
        }
    }

    fn level(self, age: u64, gate: u64) -> f64 {
        if gate == 0 {
            return 0.0;
        }
        if age < gate {
            return self.held_level(age);
        }
        let released = age - gate;
        if released >= self.release {
            return 0.0;
        }
        self.held_level(gate) * (1.0 - released as f64 / self.release as f64)
    }
}

/// Shared immutable acyclic oscillator graph. No envelope or velocity policy.
#[derive(Debug, Clone, Copy)]
struct PreparedFmCore {
    steps: [u64; OPERATORS],
    configuration: PreparedFmConfiguration,
}

#[derive(Debug, Clone, Copy)]
struct PreparedFmConfiguration {
    phases: [u64; OPERATORS],
    levels: [f64; OPERATORS],
    routing: Routing,
}

impl PreparedFmCore {
    fn new(
        sample_rate: u32,
        pitch: u8,
        oscillators: [Oscillator; OPERATORS],
        routing: Routing,
    ) -> Result<Self, Invalid> {
        let steps = prepared_steps(
            sample_rate,
            pitch,
            &oscillators.map(|oscillator| oscillator.ratio),
        )?;
        Ok(Self {
            steps,
            configuration: PreparedFmConfiguration::new(oscillators, routing),
        })
    }

    #[inline]
    fn routed_output<const SHAPE_OPERATORS: bool>(
        &self,
        table: &SineTable,
        age_frames: u64,
        operator_amplitudes: [f64; OPERATORS],
    ) -> f64 {
        self.configuration.routed_output::<SHAPE_OPERATORS>(
            table,
            age_frames,
            &self.steps,
            operator_amplitudes,
        )
    }

    #[inline]
    fn routed_output_with_waveforms(
        &self,
        sine_table: &SineTable,
        waveform_bank: &OperatorWaveformBank,
        age_frames: u64,
        operator_amplitudes: [f64; OPERATORS],
        waveform_data: (&[OperatorWaveform; OPERATORS], &[u8; OPERATORS]),
    ) -> f64 {
        self.configuration.routed_output_with_waveforms(
            sine_table,
            waveform_bank,
            age_frames,
            &self.steps,
            operator_amplitudes,
            waveform_data,
        )
    }
}

fn prepared_steps(
    sample_rate: u32,
    pitch: u8,
    ratios: &[f64; OPERATORS],
) -> Result<[u64; OPERATORS], Invalid> {
    if !(8_000..=192_000).contains(&sample_rate) || pitch > 127 {
        return Err(Invalid("invalid fm sample rate or pitch"));
    }
    let frequency = 440.0 * 2.0_f64.powf((f64::from(pitch) - 69.0) / 12.0);
    prepared_steps_frequency(sample_rate, frequency, ratios)
}
fn prepared_steps_frequency(
    sample_rate: u32,
    frequency: f64,
    ratios: &[f64; OPERATORS],
) -> Result<[u64; OPERATORS], Invalid> {
    if !(8_000..=192_000).contains(&sample_rate) || !frequency.is_finite() || frequency <= 0.0 {
        return Err(Invalid("invalid fm sample rate or frequency"));
    }
    let mut steps = [0; OPERATORS];
    for (index, ratio) in ratios.iter().enumerate() {
        let frequency = frequency * ratio;
        if !frequency.is_finite() || frequency <= 0.0 || frequency >= f64::from(sample_rate) * 0.5 {
            return Err(Invalid("fm oscillator frequency must be below nyquist"));
        }
        steps[index] = crate::quantized_phase_step(sample_rate, frequency)?;
    }
    Ok(steps)
}

impl PreparedFmConfiguration {
    fn new(oscillators: [Oscillator; OPERATORS], routing: Routing) -> Self {
        let mut phases = [0; OPERATORS];
        let mut levels = [0.0; OPERATORS];
        for (index, oscillator) in oscillators.iter().enumerate() {
            phases[index] = phase_from_turns(oscillator.phase_turns);
            levels[index] = oscillator.level;
        }
        // Pruning does not skip validation: note preparation checks every
        // oscillator's frequency, including inaudible operators. Existing
        // level slots encode the result, preserving the inline note ABI.
        let audible = audible_operator_ancestors(&levels, &routing);
        for (index, is_audible) in audible.into_iter().enumerate() {
            if !is_audible {
                levels[index] = 0.0;
            }
        }
        Self {
            phases,
            levels,
            routing,
        }
    }

    #[inline]
    fn routed_output<const SHAPE_OPERATORS: bool>(
        &self,
        table: &SineTable,
        age_frames: u64,
        steps: &[u64; OPERATORS],
        operator_amplitudes: [f64; OPERATORS],
    ) -> f64 {
        self.routed_output_with_feedback::<SHAPE_OPERATORS>(
            table,
            age_frames,
            steps,
            operator_amplitudes,
            [0.0; OPERATORS],
        )
        .0
    }

    #[inline]
    fn routed_output_with_feedback<const SHAPE_OPERATORS: bool>(
        &self,
        table: &SineTable,
        age_frames: u64,
        steps: &[u64; OPERATORS],
        operator_amplitudes: [f64; OPERATORS],
        feedback_turns: [f64; OPERATORS],
    ) -> (f64, [f64; OPERATORS]) {
        let mut signals = [0.0; OPERATORS];
        for &target in &self.routing.order {
            if self.levels[target] == 0.0 {
                continue;
            }
            let mut modulation: f64 = signals
                .iter()
                .enumerate()
                .map(|(source, signal)| signal * self.routing.modulation[source][target])
                .sum();
            if feedback_turns[target] != 0.0 {
                modulation += feedback_turns[target];
            }
            let phase = steps[target]
                .wrapping_mul(age_frames)
                .wrapping_add(self.phases[target])
                .wrapping_add(phase_from_turns(modulation));
            let signal = f64::from(table.sample_phase(phase)) * self.levels[target];
            signals[target] = if SHAPE_OPERATORS {
                signal * operator_amplitudes[target]
            } else {
                signal
            };
        }
        let output = signals
            .iter()
            .zip(self.routing.carriers)
            .map(|(signal, weight)| signal * weight)
            .sum();
        (output, signals)
    }

    #[inline]
    fn routed_output_with_waveforms(
        &self,
        sine_table: &SineTable,
        waveform_bank: &OperatorWaveformBank,
        age_frames: u64,
        steps: &[u64; OPERATORS],
        operator_amplitudes: [f64; OPERATORS],
        waveform_data: (&[OperatorWaveform; OPERATORS], &[u8; OPERATORS]),
    ) -> f64 {
        self.routed_output_with_waveforms_and_feedback(
            sine_table,
            waveform_bank,
            (age_frames, steps),
            operator_amplitudes,
            waveform_data,
            [0.0; OPERATORS],
        )
        .0
    }

    #[inline]
    fn routed_output_with_waveforms_and_feedback(
        &self,
        sine_table: &SineTable,
        waveform_bank: &OperatorWaveformBank,
        phase_data: (u64, &[u64; OPERATORS]),
        operator_amplitudes: [f64; OPERATORS],
        waveform_data: (&[OperatorWaveform; OPERATORS], &[u8; OPERATORS]),
        feedback_turns: [f64; OPERATORS],
    ) -> (f64, [f64; OPERATORS]) {
        let (age_frames, steps) = phase_data;
        let (waveforms, harmonic_caps) = waveform_data;
        let mut signals = [0.0; OPERATORS];
        for &target in &self.routing.order {
            if self.levels[target] == 0.0 {
                continue;
            }
            let mut modulation: f64 = signals
                .iter()
                .enumerate()
                .map(|(source, signal)| signal * self.routing.modulation[source][target])
                .sum();
            if feedback_turns[target] != 0.0 {
                modulation += feedback_turns[target];
            }
            let phase = steps[target]
                .wrapping_mul(age_frames)
                .wrapping_add(self.phases[target])
                .wrapping_add(phase_from_turns(modulation));
            let oscillator = match waveforms[target] {
                OperatorWaveform::Sine => sine_table.sample_phase(phase),
                waveform => waveform_bank.sample_frame(
                    waveform,
                    harmonic_caps[target],
                    phase,
                    age_frames,
                    crate::operator_waveform::noise_stream(steps[target], target),
                ),
            };
            let signal = f64::from(oscillator) * self.levels[target];
            signals[target] = signal * operator_amplitudes[target];
        }
        let output = signals
            .iter()
            .zip(self.routing.carriers)
            .map(|(signal, weight)| signal * weight)
            .sum();
        (output, signals)
    }
}

/// One immutable, fully prepared note. Contains no heap owners or shared state.
/// The host supplies authored note age and gate duration in frames. The same
/// sample is available directly after a seek without replay for this kernel.
#[derive(Debug, Clone, Copy)]
pub struct PreparedFm4 {
    core: PreparedFmCore,
    envelope: PreparedEnvelope,
    velocity: f64,
}

impl PreparedFm4 {
    pub fn new(
        sample_rate: u32,
        pitch: u8,
        velocity: u8,
        oscillators: [Oscillator; OPERATORS],
        routing: Routing,
        envelope: Envelope,
    ) -> Result<Self, Invalid> {
        if !(8_000..=192_000).contains(&sample_rate)
            || pitch > 127
            || !(1..=127).contains(&velocity)
        {
            return Err(Invalid("invalid fm sample rate, pitch or note-on velocity"));
        }
        let core = PreparedFmCore::new(sample_rate, pitch, oscillators, routing)?;
        Ok(Self::from_core(sample_rate, velocity, core, envelope))
    }
    fn new_with_frequency(
        sample_rate: u32,
        frequency: f64,
        velocity: u8,
        oscillators: [Oscillator; OPERATORS],
        routing: Routing,
        envelope: Envelope,
    ) -> Result<Self, Invalid> {
        if !(1..=127).contains(&velocity) {
            return Err(Invalid("invalid fm note-on velocity"));
        }
        let core = PreparedFmCore {
            steps: prepared_steps_frequency(sample_rate, frequency, &oscillators.map(|o| o.ratio))?,
            configuration: PreparedFmConfiguration::new(oscillators, routing),
        };
        Ok(Self::from_core(sample_rate, velocity, core, envelope))
    }
    fn from_core(sample_rate: u32, velocity: u8, core: PreparedFmCore, envelope: Envelope) -> Self {
        // Finite, validated times/rate guarantee bounded conversions and sums.
        let frames = |seconds: f64| (seconds * f64::from(sample_rate)).round() as u64;
        Self {
            core,
            envelope: PreparedEnvelope {
                attack: frames(envelope.attack_seconds),
                decay: frames(envelope.decay_seconds),
                sustain: envelope.sustain,
                release: frames(envelope.release_seconds),
            },
            velocity: f64::from(velocity) / 127.0,
        }
    }

    pub const fn release_frames(&self) -> u64 {
        self.envelope.release
    }

    /// Pack this validated standalone note into the bounded immutable analytic
    /// sampler ABI used by the registered FM4 instrument. The shared table is
    /// initialized on this control-side
    /// call so the returned sampler never initializes it while rendering.
    /// Words are: 4 steps, 4 phases, 4 levels, 16 row-major modulation values,
    /// 4 carrier values, packed topological order, 4 continuation phase offsets,
    /// attack, decay, sustain and velocity. Release lives only in the header. Integers stay integers; floating-point fields use
    /// their exact IEEE-754 bit patterns.
    pub fn into_analytic_note(self) -> PreparedAnalyticNote {
        let _ = crate::shared_sine_table();
        let mut payload = [0; ANALYTIC_NOTE_PAYLOAD_WORDS];
        payload[0..4].copy_from_slice(&self.core.steps);
        payload[4..8].copy_from_slice(&self.core.configuration.phases);
        for (target, level) in payload[8..12]
            .iter_mut()
            .zip(self.core.configuration.levels)
        {
            *target = level.to_bits();
        }
        for source in 0..OPERATORS {
            for target in 0..OPERATORS {
                payload[12 + source * OPERATORS + target] =
                    self.core.configuration.routing.modulation[source][target].to_bits();
            }
        }
        for (target, carrier) in payload[28..32]
            .iter_mut()
            .zip(self.core.configuration.routing.carriers)
        {
            *target = carrier.to_bits();
        }
        for (slot, order) in self
            .core
            .configuration
            .routing
            .order
            .into_iter()
            .enumerate()
        {
            payload[32] |= (order as u64) << (slot * 8);
        }
        // Words33..37 are per-occurrence phase offsets, initially zero.
        payload[37] = self.envelope.attack;
        payload[38] = self.envelope.decay;
        payload[39] = self.envelope.sustain.to_bits();
        payload[40] = self.velocity.to_bits();
        PreparedAnalyticNote::new(payload, self.envelope.release, sample_fm4_note)
            .with_continuation(&FM4_CONTINUATION)
    }

    /// No limiter, feedback or antialiasing: peak is bounded by the sum of
    /// carrier levels (at most 32). Supplied table is prepared off the callback.
    pub fn sample(&self, table: &SineTable, age_frames: u64, gate_frames: u64) -> f32 {
        let amplitude = self.envelope.level(age_frames, gate_frames) * self.velocity;
        if amplitude == 0.0 {
            return 0.0;
        }
        let output = self
            .core
            .routed_output::<false>(table, age_frames, [1.0; OPERATORS]);
        (output * amplitude) as f32
    }
}

fn audible_operator_ancestors(levels: &[f64; OPERATORS], routing: &Routing) -> [bool; OPERATORS] {
    let mut audible = std::array::from_fn(|operator| {
        routing.carriers[operator] != 0.0 && levels[operator] != 0.0
    });
    for &target in routing.order.iter().rev() {
        if !audible[target] {
            continue;
        }
        for source in 0..OPERATORS {
            if levels[source] != 0.0 && routing.modulation[source][target] != 0.0 {
                audible[source] = true;
            }
        }
    }
    audible
}

static FM4_CONTINUATION: hex_lattice::AnalyticNoteContinuation =
    hex_lattice::AnalyticNoteContinuation::new(continue_fm4_note);
pub(super) fn continue_fm4_note(
    source: &[u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
    target: &[u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
    age: u64,
) -> Result<[u64; ANALYTIC_NOTE_PAYLOAD_WORDS], hex_lattice_contract::DeviceContractError> {
    if source
        .iter()
        .zip(target)
        .enumerate()
        .any(|(i, (a, b))| i >= 4 && !(33..37).contains(&i) && a != b)
        || source[..4]
            .iter()
            .chain(&target[..4])
            .any(|step| *step == 0 || *step >= (1u64 << 63))
        || !valid_analytic_order(source[32])
        || !valid_analytic_order(target[32])
    {
        return Err(hex_lattice_contract::DeviceContractError::InvalidParameterValue);
    }
    let mut next = *target;
    for i in 0..4 {
        next[33 + i] =
            source[33 + i].wrapping_add(source[i].wrapping_sub(target[i]).wrapping_mul(age));
    }
    Ok(next)
}
fn valid_analytic_order(word: u64) -> bool {
    if word >> 32 != 0 {
        return false;
    }
    let mut seen = 0u8;
    for i in 0..4 {
        let index = (word >> (i * 8)) & 0xff;
        if index >= 4 || seen & (1u8 << index) != 0 {
            return false;
        }
        seen |= 1u8 << index;
    }
    seen == 15
}

fn sample_fm4_note(
    payload: &[u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
    age_frames: u64,
    gate_frames: u64,
    release_frames: u64,
    _resources: &dyn hex_lattice::DeviceResources,
) -> StereoFrame {
    let Some(table) = crate::shared_sine_table_if_prepared() else {
        return [0.0; 2];
    };
    if !valid_analytic_order(payload[32]) {
        return [0.0; 2];
    }
    let envelope = PreparedEnvelope {
        attack: payload[37],
        decay: payload[38],
        sustain: f64::from_bits(payload[39]),
        release: release_frames,
    };
    let amplitude = envelope.level(age_frames, gate_frames) * f64::from_bits(payload[40]);
    if amplitude == 0.0 {
        return [0.0; 2];
    }
    let mut signals = [0.0; OPERATORS];
    for slot in 0..OPERATORS {
        let target = ((payload[32] >> (slot * 8)) & 0xff) as usize;
        if target >= OPERATORS {
            return [0.0; 2];
        }
        let level = f64::from_bits(payload[8 + target]);
        if level == 0.0 {
            continue;
        }
        let modulation: f64 = signals
            .iter()
            .enumerate()
            .map(|(source, signal)| {
                signal * f64::from_bits(payload[12 + source * OPERATORS + target])
            })
            .sum();
        let phase = payload[target]
            .wrapping_mul(age_frames)
            .wrapping_add(payload[4 + target])
            .wrapping_add(payload[33 + target])
            .wrapping_add(phase_from_turns(modulation));
        signals[target] = f64::from(table.sample_phase(phase)) * level;
    }
    let output: f64 = signals
        .iter()
        .enumerate()
        .map(|(index, signal)| signal * f64::from_bits(payload[28 + index]))
        .sum();
    let sample = (output * amplitude) as f32;
    [sample, sample]
}

fn phase_from_turns(turns: f64) -> u64 {
    (turns.rem_euclid(1.0) * PHASE_SCALE).round() as u64
}

#[cfg(test)]
mod tests;
