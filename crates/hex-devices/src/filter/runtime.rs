//! Fixed-inline filter history, stereo LFO phase/spin/S&H, beat-sync clock
//! integration, keyed peak+hold envelope follower, 10 filter modes (LP/HP/BP/
//! Notch/Morph/DJ/Notch+LP/Comb/Resampling/Vowel), 5 drive circuits, bounded
//! parameter automation/modulation, and deterministic finite-tail drain (`#8951`).
//! The host copies committed state into scratch before each transaction.

use super::*;
use crate::auto_pan::Waveform;
use hex_dsp::{OnePoleSmoother, SvfFilter, SvfFrame, SvfMode, SvfSection, flush_quiet};
use hex_lattice::{
    DeviceResources, NativeInstrumentClock, NativeParameterAutomation, NativeProcessorRuntime,
    OutputTail, StereoFrame,
};
use std::{
    any::Any,
    f64::consts::{PI, TAU},
};

/// Worst-case resonant filter/comb tail ceiling in seconds.
const MAX_TAIL_SECONDS: u32 = 6;
/// Fixed inline comb delay line capacity per channel (zero heap allocation).
pub(super) const MAX_COMB_DELAY: usize = 512;
/// Explicit fallback tempo (120 BPM) when `NativeInstrumentClock` is unavailable.
const DEFAULT_BPM: f64 = 120.0;

fn max_tail_frames_u32(rate: u32) -> u32 {
    rate.saturating_mul(MAX_TAIL_SECONDS)
}

fn envelope_pole(rate: u32, milliseconds: f64) -> Result<f64, DeviceContractError> {
    let seconds = milliseconds * 0.001;
    if rate == 0 || !seconds.is_finite() || seconds < 0.0 {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    Ok(if seconds == 0.0 {
        1.0
    } else {
        -(-1.0 / seconds / f64::from(rate)).exp_m1()
    })
}

/// Sync division periods expressed in quarter-note beats per LFO cycle.
pub(super) fn sync_division_beats(choice: u32) -> f64 {
    match choice {
        0 => 4.0,       // 1/1
        1 => 2.0,       // 1/2
        2 => 1.0,       // 1/4
        3 => 0.5,       // 1/8
        4 => 0.25,      // 1/16
        5 => 0.125,     // 1/32
        6 => 2.0 / 3.0, // 1/4t
        7 => 1.0 / 3.0, // 1/8t
        8 => 1.5,       // 1/4d
        9 => 0.75,      // 1/8d
        _ => 1.0,
    }
}

/// Interpolated 3-formant vowel frequencies (Hz) across A / E / I / O / U.
pub(super) fn vowel_formants_hz(morph: f64, cutoff_hz: f64) -> [f64; 3] {
    const TABLE: [[f64; 3]; 5] = [
        [730.0, 1090.0, 2440.0], // a
        [530.0, 1840.0, 2480.0], // e
        [270.0, 2290.0, 3010.0], // i
        [570.0, 840.0, 2410.0],  // o
        [300.0, 870.0, 2240.0],  // u
    ];
    let pos = morph.clamp(0.0, 1.0) * 4.0;
    let idx0 = (pos.floor() as usize).min(4);
    let idx1 = (idx0 + 1).min(4);
    let frac = pos - idx0 as f64;
    let scale = (cutoff_hz / 1000.0).clamp(0.25, 4.0);
    [
        (TABLE[idx0][0] + (TABLE[idx1][0] - TABLE[idx0][0]) * frac) * scale,
        (TABLE[idx0][1] + (TABLE[idx1][1] - TABLE[idx0][1]) * frac) * scale,
        (TABLE[idx0][2] + (TABLE[idx1][2] - TABLE[idx0][2]) * frac) * scale,
    ]
}

/// Apply circuit-specific drive saturation (0 = clean bypasses nonlinearity when drive == 0).
#[inline]
fn apply_circuit_drive(sample: f64, circuit: u32, drive_lin: f64) -> f64 {
    if circuit == 0 && (drive_lin - 1.0).abs() < 1e-6 {
        return sample;
    }
    let driven = sample * drive_lin;
    match circuit {
        // clean: pure linear drive gain with gentle safety ceiling
        0 => driven,
        // osr: smooth symmetric soft saturation
        1 => (driven * 1.15).tanh() / 1.15,
        // ms2: asymmetric diode-style clipping
        2 => {
            if driven >= 0.0 {
                (driven * 1.35).tanh() / 1.35
            } else {
                driven / (1.0 + driven.abs() * 0.65)
            }
        }
        // smp: warm even-harmonic bias + tanh coloration
        3 => ((driven + 0.08 * driven * driven).tanh()) * 0.96,
        // prd: punchy ladder-style pre-saturation
        4 => {
            let x = driven * 1.25;
            x / (1.0 + x * x).sqrt()
        }
        _ => driven,
    }
}

/// Build the primary SVF frame for any filter mode (`0..=9`).
pub(super) fn build_frame(settings: Settings, rate: u32) -> Result<SvfFrame, DeviceContractError> {
    let sample_rate = f64::from(rate);
    let frequency = settings.float(CUTOFF).min(0.45 * sample_rate);
    let res = settings.float(RESONANCE) / 100.0;
    let morph = settings.float(MORPH) / 100.0;
    let mix = settings.float(MIX) / 100.0;
    let slope = f64::from(settings.0[SLOPE]);
    let mode_idx = settings.0[TYPE];
    let (svf_mode, eff_freq, eff_morph) = match mode_idx {
        0..=4 => (
            SvfMode::from_index(mode_idx)
                .map_err(|_| DeviceContractError::InvalidParameterValue)?,
            frequency,
            morph,
        ),
        // DJ mode: bipolar LP (<50%) -> neutral (50%) -> HP (>50%) tilt
        5 => {
            if morph < 0.495 {
                let norm = (morph / 0.495).clamp(0.0, 1.0);
                let dj_hz =
                    (20.0 * (frequency.max(40.0) / 20.0).powf(norm)).min(0.45 * sample_rate);
                (SvfMode::LowPass, dj_hz, morph)
            } else if morph > 0.505 {
                let norm = ((morph - 0.505) / 0.495).clamp(0.0, 1.0);
                let dj_hz = (frequency.max(40.0) * (12_000.0 / frequency.max(40.0)).powf(norm))
                    .min(0.45 * sample_rate);
                (SvfMode::HighPass, dj_hz, morph)
            } else {
                // Center deadband is transparent; use LP at nyquist-safe upper bound.
                (SvfMode::LowPass, 0.45 * sample_rate, 0.5)
            }
        }
        // Notch + LP: primary stage is Notch; secondary stage adds LP in runtime
        6 => (SvfMode::Notch, frequency, morph),
        // Comb / Resampling / Vowel still carry a valid SVF frame for cutoff/mix/resonance smoothing
        7..=9 => (SvfMode::LowPass, frequency, morph),
        _ => return Err(DeviceContractError::InvalidParameterValue),
    };
    Ok(SvfFrame::clean_filter(
        (PI * eff_freq / sample_rate).tan(),
        svf_mode,
        res,
        eff_morph,
        mix,
        slope,
    ))
}

#[inline]
fn next_xorshift64(state: &mut u64) -> f64 {
    let mut x = if *state == 0 {
        0x9E37_79B9_7F4A_7C15
    } else {
        *state
    };
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    // Map upper 53 bits deterministically to [-1.0, 1.0].
    let unit = ((x >> 11) as f64) * (1.0 / ((1u64 << 53) as f64));
    2.0 * unit - 1.0
}

#[derive(Clone, Copy)]
pub struct PreparedFilter {
    pub(super) settings: Settings,
    admitted: [bool; COUNT],
    pub(super) frame: SvfFrame,
    rate: u32,
    mode: u32,
    circuit: u32,
    drive_lin: f64,
    oscillator: Option<Waveform>,
    is_sample_and_hold: bool,
    lfo_sync: bool,
    lfo_sync_beats: f64,
    lfo_spin_mode: bool,
    lfo_spin_ratio: f64,
    lfo_global_offset: f64,
    phase_step: f64,
    phase_offset: f64,
    lfo_depth: f64,
    envelope_depth: f64,
    env_attack_bits: u32,
    env_attack_coeff: f64,
    env_release_bits: u32,
    env_release_coeff: f64,
    env_hold_frames: u32,
    env_key_routing: u32,
    modulation_smoother: OnePoleSmoother,
}

impl PreparedFilter {
    pub(super) fn new(
        settings: Settings,
        rate: u32,
        admitted: [bool; COUNT],
    ) -> Result<Self, DeviceContractError> {
        if !(8_000..=192_000).contains(&rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let sample_rate = f64::from(rate);
        let frame = build_frame(settings, rate)?;
        let modulation_smoother = OnePoleSmoother::new(0.0, rate, 0.002)
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        let env_attack_coeff = envelope_pole(rate, settings.float(ENV_ATTACK))?;
        let env_release_coeff = envelope_pole(rate, settings.float(ENV_RELEASE))?;
        let shape_choice = settings.0[LFO_SHAPE];
        let (oscillator, is_sample_and_hold) = if shape_choice == 5 {
            (None, true)
        } else {
            (Some(Waveform::from_choice(shape_choice)?), false)
        };
        let drive_db = settings.float(DRIVE);
        let drive_lin = 10.0_f64.powf(drive_db / 20.0);
        let env_hold_frames =
            ((settings.float(ENV_HOLD) * 0.001 * sample_rate).round() as u32).min(rate);
        Ok(Self {
            settings,
            admitted,
            frame,
            rate,
            mode: settings.0[TYPE],
            circuit: settings.0[CIRCUIT],
            drive_lin,
            oscillator,
            is_sample_and_hold,
            lfo_sync: settings.0[LFO_SYNC] != 0,
            lfo_sync_beats: sync_division_beats(settings.0[LFO_SYNC_RATE]),
            lfo_spin_mode: settings.0[LFO_STEREO_MODE] != 0,
            lfo_spin_ratio: settings.float(LFO_SPIN) / 100.0,
            lfo_global_offset: settings.float(LFO_OFFSET) / 360.0,
            phase_step: settings.float(LFO_RATE) / sample_rate,
            phase_offset: settings.float(LFO_PHASE) / 360.0,
            lfo_depth: settings.float(LFO_AMOUNT),
            envelope_depth: settings.float(ENV_AMOUNT),
            env_attack_bits: settings.0[ENV_ATTACK],
            env_attack_coeff,
            env_release_bits: settings.0[ENV_RELEASE],
            env_release_coeff,
            env_hold_frames,
            env_key_routing: settings.0[ENV_KEY_ROUTING],
            modulation_smoother,
        })
    }

    fn supports(&self, id: ParameterId) -> bool {
        parameter_index(id).is_ok_and(|index| self.admitted[index])
    }
}

impl PreparedNativeProcessor for PreparedFilter {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.supports(id)
    }
    fn parameter_automation_descriptor(&self, id: ParameterId) -> Option<ParameterDescriptor> {
        crate::parameter_binding::binding(&BINDINGS, id)
            .ok()
            .filter(|_| self.supports(id))
            .map(|binding| binding.descriptor)
    }
    fn parameter_modulation_semantics(&self, id: ParameterId) -> Option<ParameterSemantics> {
        crate::parameter_binding::binding(&BINDINGS, id)
            .ok()
            .filter(|_| self.supports(id))
            .map(|binding| binding.semantics)
    }
    fn runtime_version(&self) -> u32 {
        VERSION
    }
    fn declared_runtime_bytes(&self) -> usize {
        size_of::<FilterRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(FilterRuntime::new(*self))
    }
    fn supports_smoothed_bypass(&self) -> bool {
        true
    }
    fn supports_power_off(&self) -> bool {
        true
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite {
            max_tail_frames: u64::from(max_tail_frames_u32(self.rate)),
        }
    }
}

impl crate::inline_owned::InlinePreparedProcessor for PreparedFilter {
    type Runtime = FilterRuntime;
    fn create_inline_runtime(&self) -> Self::Runtime {
        FilterRuntime::new(*self)
    }
}

/// Copy-bounded payload charged by the sealed host storage ledger.
#[derive(Clone, Copy)]
pub struct FilterRuntime {
    base: PreparedFilter,
    controls: crate::parameter_binding::Controls<Settings, COUNT>,
    target_frame: SvfFrame,
    frame: SvfFrame,
    ramp: u32,
    drive_lin: f64,
    phase_step: f64,
    phase_offset: f64,
    lfo_global_offset: f64,
    lfo_spin_ratio: f64,
    lfo_depth: f64,
    envelope_depth: f64,
    lane: [SvfFilter; 2],
    secondary_lp: [SvfSection; 2],
    vowel_banks: [[SvfSection; 3]; 2],
    comb_buf: [[f32; MAX_COMB_DELAY]; 2],
    comb_write: usize,
    resample_hold: [f64; 2],
    resample_phase: [f64; 2],
    env_attack_bits: u32,
    env_attack_coeff: f64,
    env_release_bits: u32,
    env_release_coeff: f64,
    env_hold_frames: u32,
    env_hold_remaining: u32,
    env_level: f64,
    phase_l: f64,
    phase_r: f64,
    sh_rng: [u64; 2],
    sh_value: [f64; 2],
    modulation: [OnePoleSmoother; 2],
    silent_frames: u32,
}

impl FilterRuntime {
    fn new(base: PreparedFilter) -> Self {
        let mut rng_l = 0xD1B5_4A32_D192_ED03u64;
        let mut rng_r = 0x94D0_49BB_1331_11EBu64;
        let init_l = next_xorshift64(&mut rng_l);
        let init_r = next_xorshift64(&mut rng_r);
        Self {
            base,
            controls: crate::parameter_binding::Controls::new(base.settings, base.admitted),
            target_frame: base.frame,
            frame: base.frame,
            ramp: 0,
            drive_lin: base.drive_lin,
            phase_step: base.phase_step,
            phase_offset: base.phase_offset,
            lfo_global_offset: base.lfo_global_offset,
            lfo_spin_ratio: base.lfo_spin_ratio,
            lfo_depth: base.lfo_depth,
            envelope_depth: base.envelope_depth,
            lane: [SvfFilter::new(); 2],
            secondary_lp: [SvfSection::new(); 2],
            vowel_banks: [[SvfSection::new(); 3]; 2],
            comb_buf: [[0.0; MAX_COMB_DELAY]; 2],
            comb_write: 0,
            resample_hold: [0.0; 2],
            resample_phase: [0.0; 2],
            env_attack_bits: base.env_attack_bits,
            env_attack_coeff: base.env_attack_coeff,
            env_release_bits: base.env_release_bits,
            env_release_coeff: base.env_release_coeff,
            env_hold_frames: base.env_hold_frames,
            env_hold_remaining: 0,
            env_level: 0.0,
            phase_l: base.lfo_global_offset.fract(),
            phase_r: (base.lfo_global_offset + base.phase_offset).fract(),
            sh_rng: [rng_l, rng_r],
            sh_value: [init_l, init_r],
            modulation: [base.modulation_smoother; 2],
            silent_frames: max_tail_frames_u32(base.rate),
        }
    }

    fn clear_history(&mut self) {
        self.lane = [SvfFilter::new(); 2];
        self.secondary_lp = [SvfSection::new(); 2];
        self.vowel_banks = [[SvfSection::new(); 3]; 2];
        self.comb_buf = [[0.0; MAX_COMB_DELAY]; 2];
        self.comb_write = 0;
        self.resample_hold = [0.0; 2];
        self.resample_phase = [0.0; 2];
        self.env_level = 0.0;
        self.env_hold_remaining = 0;
        self.modulation = [self.base.modulation_smoother; 2];
    }

    fn retarget_from_settings(
        &mut self,
        settings: Settings,
        previous: Settings,
        immediate_frame: bool,
    ) -> Result<(), DeviceContractError> {
        let rate = self.base.rate;
        let sample_rate = f64::from(rate);
        if settings.0[CUTOFF] != previous.0[CUTOFF]
            || settings.0[RESONANCE] != previous.0[RESONANCE]
            || settings.0[MORPH] != previous.0[MORPH]
            || settings.0[MIX] != previous.0[MIX]
        {
            let next_frame = build_frame(settings, rate)?;
            if immediate_frame {
                self.frame = next_frame;
                self.target_frame = next_frame;
                self.ramp = 0;
            } else if next_frame != self.target_frame {
                self.target_frame = next_frame;
                self.ramp = rate.div_ceil(200);
            }
        }
        self.drive_lin = 10.0_f64.powf(settings.float(DRIVE) / 20.0);
        self.phase_step = settings.float(LFO_RATE) / sample_rate;
        self.phase_offset = settings.float(LFO_PHASE) / 360.0;
        self.lfo_global_offset = settings.float(LFO_OFFSET) / 360.0;
        self.lfo_spin_ratio = settings.float(LFO_SPIN) / 100.0;
        self.lfo_depth = settings.float(LFO_AMOUNT);
        self.envelope_depth = settings.float(ENV_AMOUNT);
        self.env_hold_frames =
            ((settings.float(ENV_HOLD) * 0.001 * sample_rate).round() as u32).min(rate);
        if settings.0[ENV_ATTACK] != self.env_attack_bits {
            self.env_attack_coeff = envelope_pole(rate, settings.float(ENV_ATTACK))?;
            self.env_attack_bits = settings.0[ENV_ATTACK];
        }
        if settings.0[ENV_RELEASE] != self.env_release_bits {
            self.env_release_coeff = envelope_pole(rate, settings.float(ENV_RELEASE))?;
            self.env_release_bits = settings.0[ENV_RELEASE];
        }
        Ok(())
    }

    #[inline]
    fn detector_peak(&self, input: [f32; 2]) -> f64 {
        let l = f64::from(input[0]);
        let r = f64::from(input[1]);
        let raw = match self.base.env_key_routing {
            0 => l.abs().max(r.abs()),  // stereo linked
            1 => l.abs(),               // left
            2 => r.abs(),               // right
            3 => (0.5 * (l + r)).abs(), // mid
            4 => (0.5 * (l - r)).abs(), // side
            _ => l.abs().max(r.abs()),
        };
        raw.min(1.0)
    }

    #[inline]
    fn lfo_step_for_clock(&self, clock: Option<&dyn NativeInstrumentClock>, offset: usize) -> f64 {
        if !self.base.lfo_sync {
            return self.phase_step;
        }
        let bpm = clock
            .and_then(|c| c.tempo_micro_bpm_at_offset(offset))
            .map(|u| f64::from(u) / 1_000_000.0)
            .filter(|b| b.is_finite() && *b >= 20.0 && *b <= 999.0)
            .unwrap_or(DEFAULT_BPM);
        let beats_per_sec = bpm / 60.0;
        let hz = beats_per_sec / self.base.lfo_sync_beats.max(1e-6);
        hz / f64::from(self.base.rate)
    }

    fn advance_lfo_phases(&mut self, step_l: f64) {
        let step_r = if self.base.lfo_spin_mode {
            step_l * (1.0 + self.lfo_spin_ratio)
        } else {
            step_l
        };
        let next_l = self.phase_l + step_l;
        if next_l >= 1.0 {
            self.sh_value[0] = next_xorshift64(&mut self.sh_rng[0]);
        }
        self.phase_l = next_l.fract();

        if self.base.lfo_spin_mode {
            let next_r = self.phase_r + step_r;
            if next_r >= 1.0 {
                self.sh_value[1] = next_xorshift64(&mut self.sh_rng[1]);
            }
            self.phase_r = next_r.fract();
        } else {
            let prev_r = (self.phase_l - step_l + self.phase_offset).rem_euclid(1.0);
            let cur_r = (self.phase_l + self.phase_offset).fract();
            if cur_r < prev_r {
                self.sh_value[1] = next_xorshift64(&mut self.sh_rng[1]);
            }
            self.phase_r = cur_r;
        }
    }

    fn process_frame(
        &mut self,
        frame: &mut StereoFrame,
        clock: Option<&dyn NativeInstrumentClock>,
        offset: usize,
    ) {
        if self.ramp != 0 {
            self.frame = self
                .frame
                .interpolate(self.target_frame, 1.0 / f64::from(self.ramp));
            self.ramp -= 1;
        }
        let step_l = self.lfo_step_for_clock(clock, offset);
        let input = frame.map(|sample| if sample.is_finite() { sample } else { 0.0 });
        let limit = max_tail_frames_u32(self.base.rate);
        let silent_input = input == [0.0; 2];
        if silent_input {
            if self.silent_frames >= limit {
                self.advance_lfo_phases(step_l);
                *frame = [0.0; 2];
                return;
            }
            self.silent_frames = self.silent_frames.saturating_add(1).min(limit);
            if self.silent_frames == limit {
                self.clear_history();
                self.advance_lfo_phases(step_l);
                *frame = [0.0; 2];
                return;
            }
        } else {
            self.silent_frames = 0;
        }

        // Envelope follower with explicit peak-hold window before release.
        let peak = self.detector_peak(input);
        if peak > self.env_level {
            self.env_level =
                flush_quiet(self.env_level + (peak - self.env_level) * self.env_attack_coeff);
            self.env_hold_remaining = self.env_hold_frames;
        } else if self.env_hold_remaining > 0 {
            self.env_hold_remaining -= 1;
        } else {
            self.env_level =
                flush_quiet(self.env_level + (peak - self.env_level) * self.env_release_coeff);
        }
        let envelope = f64::from(self.env_level as f32) * self.envelope_depth;

        let fade_frames = self.base.rate.div_ceil(200).max(1);
        let remaining = limit.saturating_sub(self.silent_frames);
        let tail_gain = if silent_input && remaining < fade_frames {
            f64::from(remaining) / f64::from(fade_frames)
        } else {
            1.0
        };

        let effective_settings = self.controls.effective();
        let base_cutoff = effective_settings.float(CUTOFF);
        let res_norm = effective_settings.float(RESONANCE) / 100.0;
        let morph_norm = effective_settings.float(MORPH) / 100.0;
        let mix_norm = self.frame.wet();
        let sample_rate = f64::from(self.base.rate);

        for (channel, sample) in frame.iter_mut().enumerate() {
            let lfo = if self.lfo_depth == 0.0 {
                0.0
            } else if self.base.is_sample_and_hold {
                self.sh_value[channel] * self.lfo_depth
            } else {
                let phase = if channel == 0 {
                    self.phase_l
                } else {
                    self.phase_r
                };
                let wave = self.base.oscillator.map_or(0.5, |osc| osc.lfo(phase));
                (2.0 * wave - 1.0) * self.lfo_depth
            };
            let mod_oct = self.modulation[channel].advance_toward(envelope + lfo);
            let dry = f64::from(input[channel]);
            let driven = apply_circuit_drive(dry, self.base.circuit, self.drive_lin);
            let mod_cutoff = (base_cutoff * 2.0_f64.powf(mod_oct)).clamp(20.0, 0.45 * sample_rate);

            let processed = match self.base.mode {
                // Modes 0..=5: TPT SVF core (LP, HP, BP, Notch, Morph, DJ)
                0..=5 => {
                    let coefficients = if mod_oct.abs() > 1e-6 {
                        self.frame.with_cutoff_offset(mod_oct, self.base.rate)
                    } else {
                        self.modulation[channel].reset(0.0);
                        self.frame
                    };
                    let wet_only = self.lane[channel].process(driven, coefficients.with_full_wet());
                    dry * (1.0 - mix_norm) + wet_only * mix_norm
                }
                // Mode 6: Notch + Low-Pass dual stage
                6 => {
                    let coefficients = if mod_oct.abs() > 1e-6 {
                        self.frame.with_cutoff_offset(mod_oct, self.base.rate)
                    } else {
                        self.modulation[channel].reset(0.0);
                        self.frame
                    };
                    let notched = self.lane[channel].process(driven, coefficients.with_full_wet());
                    let lp_cutoff =
                        (mod_cutoff * (1.25 + 2.75 * morph_norm)).clamp(20.0, 0.45 * sample_rate);
                    let lp_g = (PI * lp_cutoff / sample_rate).tan();
                    let lp_out = self.secondary_lp[channel]
                        .process_outputs(notched, lp_g, 1.0 / (0.707 + 2.5 * res_norm))
                        .low;
                    dry * (1.0 - mix_norm) + lp_out * mix_norm
                }
                // Mode 7: Inline feedback Comb filter (zero allocation)
                7 => {
                    let delay_f = (sample_rate / mod_cutoff.max(96.0))
                        .clamp(2.0, (MAX_COMB_DELAY - 2) as f64);
                    let d0 = delay_f.floor() as usize;
                    let frac = delay_f - d0 as f64;
                    let idx0 = (self.comb_write + MAX_COMB_DELAY - d0) % MAX_COMB_DELAY;
                    let idx1 = (idx0 + MAX_COMB_DELAY - 1) % MAX_COMB_DELAY;
                    let delayed = f64::from(self.comb_buf[channel][idx0]) * (1.0 - frac)
                        + f64::from(self.comb_buf[channel][idx1]) * frac;
                    let feedback = (0.15 + 0.78 * res_norm).min(0.93);
                    let polarity = if morph_norm < 0.5 { 1.0 } else { -1.0 };
                    let y = flush_quiet(driven + polarity * feedback * delayed);
                    self.comb_buf[channel][self.comb_write] =
                        y.clamp(f64::from(f32::MIN), f64::from(f32::MAX)) as f32;
                    dry * (1.0 - mix_norm) + y * mix_norm
                }
                // Mode 8: Resampling zero-order-hold clocked at cutoff Hz
                8 => {
                    let step = (mod_cutoff / sample_rate).clamp(0.0005, 1.0);
                    self.resample_phase[channel] += step;
                    if self.resample_phase[channel] >= 1.0 {
                        self.resample_phase[channel] -= self.resample_phase[channel].floor();
                        self.resample_hold[channel] = driven;
                    }
                    let wet_out = self.resample_hold[channel];
                    dry * (1.0 - mix_norm) + wet_out * mix_norm
                }
                // Mode 9: Vowel 3-formant parallel SVF resonator bank
                9 => {
                    let formants = vowel_formants_hz(morph_norm, mod_cutoff);
                    let q = 3.0 + 10.0 * res_norm;
                    let k = 1.0 / q;
                    let gains = [1.0, 0.75, 0.5];
                    let mut sum = 0.0;
                    for (band_idx, &hz) in formants.iter().enumerate() {
                        let clamped = hz.clamp(40.0, 0.45 * sample_rate);
                        let g = (PI * clamped / sample_rate).tan();
                        let bp = self.vowel_banks[channel][band_idx]
                            .process_outputs(driven, g, k)
                            .band;
                        sum += bp * gains[band_idx];
                    }
                    dry * (1.0 - mix_norm) + sum * mix_norm
                }
                _ => dry,
            };

            let output = processed * tail_gain;
            *sample = output.clamp(f64::from(f32::MIN), f64::from(f32::MAX)) as f32;
        }
        self.comb_write = (self.comb_write + 1) % MAX_COMB_DELAY;

        if silent_input
            && self.base.mode != 7
            && self.lane.into_iter().all(SvfFilter::is_quiet)
            && frame[0] == 0.0
            && frame[1] == 0.0
        {
            self.clear_history();
            self.silent_frames = limit;
        }
        self.advance_lfo_phases(step_l);
    }

    fn process_automated(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        values: &dyn NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        let mut resolved = self.controls.resolve(&BINDINGS, values)?;
        let sr_cutoff = values.has_sample_rate_modulation(DESCRIPTORS[CUTOFF].id);
        let sr_resonance = values.has_sample_rate_modulation(DESCRIPTORS[RESONANCE].id);
        let sr_morph = values.has_sample_rate_modulation(DESCRIPTORS[MORPH].id);
        let sr_mix = values.has_sample_rate_modulation(DESCRIPTORS[MIX].id);
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) = self
                .controls
                .sample(&BINDINGS, &mut resolved, values, offset)?
            {
                let old = self.controls.effective();
                let next_effective = next.effective();
                let immediate_frame = (next_effective.0[CUTOFF] != old.0[CUTOFF] && sr_cutoff)
                    || (next_effective.0[RESONANCE] != old.0[RESONANCE] && sr_resonance)
                    || (next_effective.0[MORPH] != old.0[MORPH] && sr_morph)
                    || (next_effective.0[MIX] != old.0[MIX] && sr_mix);
                self.retarget_from_settings(next_effective, old, immediate_frame)?;
                self.controls.install(next);
            }
            let original = *frame;
            self.process_frame(frame, clock, offset);
            if bypassed {
                *frame = original;
            }
        }
        Ok(())
    }
}

impl NativeProcessorRuntime for FilterRuntime {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.controls.supports(&BINDINGS, id)
    }
    crate::parameter_binding::delegate_automated_block_processing!(with_clock process_automated);

    fn process_block(&mut self, frames: &mut [StereoFrame], resources: &dyn DeviceResources) {
        self.process_block_with_clock(frames, None, resources);
    }

    fn process_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn DeviceResources,
    ) {
        for (offset, frame) in frames.iter_mut().enumerate() {
            self.process_frame(frame, clock, offset);
        }
    }

    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn DeviceResources,
    ) {
        for (offset, frame) in frames.iter_mut().enumerate() {
            let original = *frame;
            self.process_frame(frame, clock, offset);
            *frame = original;
        }
    }

    fn reset(&mut self) {
        *self = Self::new(self.base);
    }
    fn is_quiescent(&self) -> bool {
        self.lane.into_iter().all(SvfFilter::is_quiet)
            && self.silent_frames >= max_tail_frames_u32(self.base.rate)
    }
    fn can_copy_history_from(&self, old: &dyn NativeProcessorRuntime) -> bool {
        old.as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| old.base.rate == self.base.rate)
    }
    fn can_retarget_parameters_from(&self, old: &dyn NativeProcessorRuntime) -> bool {
        self.can_copy_history_from(old)
    }
    fn can_retarget_source_parameters_from(&self, old: &dyn NativeProcessorRuntime) -> bool {
        self.can_copy_history_from(old)
    }
    fn copy_history_from(&mut self, old: &dyn NativeProcessorRuntime) {
        if let Some(old) = old
            .as_any()
            .downcast_ref::<Self>()
            .filter(|old| old.base.rate == self.base.rate)
        {
            self.lane = old.lane;
            self.secondary_lp = old.secondary_lp;
            self.vowel_banks = old.vowel_banks;
            self.comb_buf = old.comb_buf;
            self.comb_write = old.comb_write;
            self.resample_hold = old.resample_hold;
            self.resample_phase = old.resample_phase;
            self.env_level = old.env_level;
            self.env_hold_remaining = old.env_hold_remaining;
            self.phase_l = old.phase_l;
            self.phase_r = old.phase_r;
            self.sh_rng = old.sh_rng;
            self.sh_value = old.sh_value;
            self.silent_frames = old.silent_frames;
            for (dst, src) in self.modulation.iter_mut().zip(&old.modulation) {
                dst.inherit_value(src);
            }
            if BINDINGS
                .iter()
                .any(|b| self.controls.supports(&BINDINGS, b.descriptor.id))
                && BINDINGS
                    .iter()
                    .any(|b| old.controls.supports(&BINDINGS, b.descriptor.id))
            {
                let previous = self.controls.effective();
                self.controls.inherit(&old.controls);
                let inherited = self.controls.effective();
                let _ = self.retarget_from_settings(inherited, previous, false);
            }
            self.frame = old.frame;
            self.ramp = if self.target_frame == old.frame {
                old.ramp
            } else {
                self.base.rate.div_ceil(200)
            };
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Evaluate analytical magnitude response at `hz` for all 10 filter modes (`#8951`).
pub(super) fn mode_magnitude_at_hz(
    prepared: &PreparedFilter,
    cutoff_octave_offset: f64,
    hz: f64,
) -> f64 {
    let rate = f64::from(prepared.rate);
    let base_cutoff = prepared.settings.float(CUTOFF);
    let mod_cutoff = (base_cutoff * 2.0_f64.powf(cutoff_octave_offset)).clamp(20.0, 0.45 * rate);
    let res_norm = prepared.settings.float(RESONANCE) / 100.0;
    let morph_norm = prepared.settings.float(MORPH) / 100.0;

    match prepared.mode {
        0..=5 => {
            let frame = if cutoff_octave_offset.abs() > 1e-6 {
                prepared
                    .frame
                    .with_cutoff_offset(cutoff_octave_offset, prepared.rate)
                    .with_full_wet()
            } else {
                prepared.frame.with_full_wet()
            };
            let compact = crate::operator_filter::Prepared {
                frame,
                rate: prepared.rate,
            };
            f64::from(compact.magnitude_at_hz(hz as f32))
        }
        6 => {
            let notch_frame = if cutoff_octave_offset.abs() > 1e-6 {
                prepared
                    .frame
                    .with_cutoff_offset(cutoff_octave_offset, prepared.rate)
                    .with_full_wet()
            } else {
                prepared.frame.with_full_wet()
            };
            let notch_mag = f64::from(
                crate::operator_filter::Prepared {
                    frame: notch_frame,
                    rate: prepared.rate,
                }
                .magnitude_at_hz(hz as f32),
            );
            let lp_cutoff = (mod_cutoff * (1.25 + 2.75 * morph_norm)).clamp(20.0, 0.45 * rate);
            let lp_frame = SvfFrame::clean_filter(
                (PI * lp_cutoff / rate).tan(),
                SvfMode::LowPass,
                res_norm * 0.5,
                0.0,
                1.0,
                0.0,
            );
            let lp_mag = f64::from(
                crate::operator_filter::Prepared {
                    frame: lp_frame,
                    rate: prepared.rate,
                }
                .magnitude_at_hz(hz as f32),
            );
            notch_mag * lp_mag
        }
        7 => {
            let delay_f = (rate / mod_cutoff.max(96.0)).clamp(2.0, (MAX_COMB_DELAY - 2) as f64);
            let fb = (0.15 + 0.78 * res_norm).min(0.93);
            let sign = if morph_norm < 0.5 { 1.0 } else { -1.0 };
            let angle = TAU * hz * delay_f / rate;
            let re = 1.0 - sign * fb * angle.cos();
            let im = sign * fb * angle.sin();
            1.0 / (re * re + im * im).sqrt().max(1e-6)
        }
        8 => {
            // Sinc-envelope magnitude for zero-order hold at `mod_cutoff` Hz.
            let x = PI * hz / mod_cutoff.max(20.0);
            if x.abs() < 1e-6 {
                1.0
            } else {
                (x.sin() / x).abs().max(1e-4)
            }
        }
        9 => {
            let formants = vowel_formants_hz(morph_norm, mod_cutoff);
            let gains = [1.0, 0.75, 0.5];
            let q = 3.0 + 10.0 * res_norm;
            let mut sum = 0.0;
            for (&f0, &gain) in formants.iter().zip(&gains) {
                let f0 = f0.clamp(40.0, 0.45 * rate);
                let ratio = hz / f0 - f0 / hz.max(1.0);
                let mag = 1.0 / (1.0 + q * q * ratio * ratio).sqrt();
                sum += gain * mag;
            }
            sum.max(1e-4)
        }
        _ => 1.0,
    }
}
