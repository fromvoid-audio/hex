//! Original limiting laws, explicitly separate from reference parity.
//!
//! A bounded monotonic maximum queue anticipates each channel's next lookahead
//! interval. Standard attack ramps linearly toward the required gain over the
//! lookahead window, retaining the steepest active ramp so later peaks cannot
//! postpone earlier deadlines. Soft-clip attack is instantaneous; true-peak
//! attack anticipates with an exponential time constant of one eighth of the
//! selected lookahead. Output constraints guard each mode's ceiling.
//! Release is exponential. In true-peak mode an additional 1 ms envelope
//! recovery floor suppresses modulation beyond the finite reconstruction band;
//! slower authored release times remain unchanged. Auto release uses
//! a 10 ms program envelope to select 50–270 ms recovery, latched while
//! recovering from attenuation. Link interpolates each
//! lane's required attenuation toward the worse lane. Soft clip first limits at
//! 1.15 times ceiling, then applies the independently reconstructed transfer
//! sign(x) * ceiling * sqrt(tanh((x / ceiling)^2)). This is an empirical fit to
//! original reference observations, not a proprietary algorithm claim. Maximize applies inverse-threshold gain before a
//! 0 dB limiter and scales afterward to Output.
//!
//! True peak detects signed, normalized 32-tap Blackman-windowed sinc
//! reconstruction at thirty-two phases. Thirty-one extra declared frames make every
//! detector window covering an outgoing sample available. Anticipated attack
//! avoids an instantaneous gain discontinuity before the transient. Independent
//! dense reconstructed-output tests cover modulation and intersample peaks;
//! this finite model does not prove every DAC's continuous-time behavior.
//! The detector reconstructs the pre-gain input, so gain that moves inside the
//! detector window can create untracked intersample peaks. On gated-burst
//! fixtures the standard BS.1770-4 4x meter reads up to about +0.25 dB above
//! the ceiling (tests bound it at +0.3 dB) and ideal reconstruction reads
//! higher; an output-aware gain path is required to remove this.
//!
//! M/s uses one independently linked encoded stage followed by a fully linked
//! l/r safety stage after decoding, so arbitrary stereo signals respect the
//! physical output ceiling. The final guard has an independent fixed 220 ms
//! recovery; authored manual/auto release belongs to the encoded stage. This
//! bounded original law fits the measured asymmetrical-to-mono reference
//! recovery and is not a claim about unpublished implementation identity.
//! Both stages' delays are declared. No final
//! hard clamp is used. Histories, queues and telemetry commit atomically; buffer
//! copies are bounded by the prepared rate/lookahead, not callback duration.
//! Metering carries the first-stage linear attenuation with its delayed sample,
//! combines the final-stage and shaper factors for that same sample, and converts
//! each process call's minimum to dB. Each factor retains the 1e-30 floor.
//!
//! The classic factory's closed transactions at the same rate retain full
//! history for a compatible layout, including silent attenuation and parameter ramps. A layout change
//! (lookahead, routing or true-peak latency) is admitted only when the donor has
//! no queued output in either stage; it explicitly resets both candidate
//! histories to the new configuration. Physical storage layouts must also match
//! before copying a nonempty history. That transition discards only silent
//! future gain/program influence. In-flight transactions and rate changes
//! refuse; the host retains its existing PDC, publication and retirement rules.
use super::automation;
use super::state::{Settings, parameter::*};
use hex_dsp::BlackmanSincBank;
use hex_lattice::{
    DynamicsMeter, OutputTail, OwnedProcessorRuntime, PreparedOwnedProcessorRuntime, StereoFrame,
};
use hex_lattice_contract::DeviceContractError;
use std::any::Any;

mod process;
mod stage;
use process::BlockStats;
use stage::{PHASES, Peak, Sample, Stage, StageDirty, TAPS, allocate, copy_cyclic};

fn reduction(attenuation: f64) -> f32 {
    hex_dsp::LinearAmplitude::new(attenuation)
        .ok()
        .and_then(hex_dsp::LinearAmplitude::decibels)
        .map_or(0.0, |db| (-db.value()).max(0.0) as f32)
}
fn meter_factor(gain: f64) -> f64 {
    if gain > 1.0 { 1.0 } else { gain.max(1e-30) }
}
fn amplitude(db: f32) -> f64 {
    hex_dsp::Decibels::new(f64::from(db))
        .and_then(hex_dsp::Decibels::linear_amplitude)
        .map_or(0.0, hex_dsp::LinearAmplitude::value)
}
/// Effective limiting targets. Derivation never rebuilds fixed reconstruction
/// coefficients or changes the prepared latency/layout. Cache and targets live
/// in each transaction history, so refusal/rollback restores both together.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Parameters {
    ceiling: f64,
    output_ceiling: f64,
    gain: f64,
    output: f64,
    release_coefficient: f64,
    auto_release: bool,
    link: f64,
}
impl Parameters {
    fn new(settings: Settings, rate: u32) -> Self {
        let maximize = settings.flag(MAXIMIZE);
        let output_ceiling = if maximize {
            1.0
        } else {
            amplitude(settings.value(CEILING))
        };
        Self {
            ceiling: output_ceiling,
            output_ceiling,
            gain: amplitude(
                settings.value(if maximize { THRESHOLD } else { GAIN })
                    * if maximize { -1.0 } else { 1.0 },
            ),
            output: if maximize {
                amplitude(settings.value(OUTPUT))
            } else {
                1.0
            },
            release_coefficient: hex_dsp::one_pole_coefficient(
                rate,
                f64::from(settings.value(RELEASE)) / 1000.0,
            )
            .unwrap_or(1.0),
            auto_release: settings.flag(AUTO_RELEASE),
            link: 0.0,
        }
        .for_layout(settings)
    }
    /// Project branch-local selectors onto rate/value coefficients already derived.
    fn for_layout(mut self, settings: Settings) -> Self {
        self.ceiling = self.output_ceiling
            * if settings.choice(MODE) == 1 {
                1.15
            } else {
                1.0
            };
        self.link = f64::from(settings.value(if settings.choice(ROUTING) == 1 {
            LINK_MID_SIDE
        } else {
            LINK
        }));
        self
    }
}
/// Fixed program data, independent of rate, state and layout. Config preparation
/// initializes this on control via `hex-dsp`; callback processing reads only its
/// stored static reference, without touching OnceLock. No host resource custody is transferred.
fn reconstruction_bank() -> &'static BlackmanSincBank<TAPS, PHASES> {
    BlackmanSincBank::shared_32x32()
}

#[derive(Clone, Copy)]
struct Config {
    rate: u32,
    inv_rate: f64,
    gain_ramp_inv: f64,
    settings: Settings,
    parameters: Parameters,
    guard_release_coefficient: f64,
    attack_coefficient: f64,
    attack_window_inverse: f64,
    antimodulation_coefficient: f64,
    program_coefficient: f64,
    true_peak: bool,
    soft_clip: bool,
    mid_side: bool,
    delay: usize,
    reconstruction: &'static BlackmanSincBank<TAPS, PHASES>,
}
impl Config {
    fn new(settings: Settings, rate: u32) -> Self {
        let true_peak = settings.choice(MODE) == 2;
        let lookahead = [1.5, 3.0, 6.0][settings.choice(LOOKAHEAD) as usize];
        let delay = (lookahead * f64::from(rate) / 1000.0).ceil() as usize
            + if true_peak { TAPS - 1 } else { 0 };
        let reconstruction = reconstruction_bank();
        let inv_rate = 1.0 / f64::from(rate);
        Self {
            rate,
            inv_rate,
            gain_ramp_inv: inv_rate / 0.005,
            settings,
            parameters: Parameters::new(settings, rate),
            antimodulation_coefficient: hex_dsp::one_pole_coefficient(rate, 0.001).unwrap_or(1.0),
            attack_coefficient: 1.0
                - (-8.0 / (delay - if true_peak { TAPS - 1 } else { 0 }) as f64).exp(),
            attack_window_inverse: 1.0 / (delay + 1) as f64,
            guard_release_coefficient: hex_dsp::one_pole_coefficient(rate, 0.22).unwrap_or(1.0),
            program_coefficient: hex_dsp::one_pole_coefficient(rate, 0.01).unwrap_or(1.0),
            true_peak,
            soft_clip: settings.choice(MODE) == 1,
            mid_side: settings.choice(ROUTING) == 1,
            delay,
            reconstruction,
        }
    }
    fn latency(self) -> u32 {
        (self.delay * if self.mid_side { 2 } else { 1 }) as u32
    }
}
/// Fixed-layout factory for the existing owned Limiter transaction.
pub struct Factory {
    config: Config,
    admitted: [bool; automation::COUNT],
}
impl Factory {
    pub(super) fn new(settings: Settings, rate: u32) -> Self {
        Self {
            config: Config::new(settings, rate),
            admitted: [false; automation::COUNT],
        }
    }
}
impl Factory {
    pub(super) fn automated(
        settings: Settings,
        rate: u32,
        admitted: [bool; automation::COUNT],
    ) -> Self {
        Self {
            config: Config::new(settings, rate),
            admitted,
        }
    }
}
impl PreparedOwnedProcessorRuntime for Factory {
    fn supports_parameter_automation(&self, id: hex_lattice_contract::ParameterId) -> bool {
        automation::slot(id).is_some_and(|i| self.admitted[i])
    }
    fn parameter_automation_descriptor(
        &self,
        id: hex_lattice_contract::ParameterId,
    ) -> Option<hex_lattice_contract::ParameterDescriptor> {
        self.supports_parameter_automation(id)
            .then(|| automation::descriptor(id))
            .flatten()
    }

    fn reserved_bypass_latency_frames(&self) -> Option<u32> {
        Some(self.config.latency())
    }
    fn supports_power_off(&self) -> bool {
        true
    }
    fn parameter_modulation_semantics(
        &self,
        id: hex_lattice_contract::ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        self.supports_parameter_automation(id)
            .then(|| automation::modulation_semantics(id))
            .flatten()
    }

    fn runtime_version(&self) -> u32 {
        2
    }
    fn declared_pair_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
            + 2 * if self.config.mid_side { 2 } else { 1 }
                * (self.config.delay + 1)
                * (std::mem::size_of::<Sample>() + 2 * std::mem::size_of::<Peak>())
    }
    fn create_runtime(&self) -> Result<Box<dyn OwnedProcessorRuntime>, DeviceContractError> {
        let runtime = Runtime {
            config: self.config,
            primary: History::new(self.config, self.admitted)?,
            scratch: History::new(self.config, self.admitted)?,
            open: false,
            releasing: false,
        };
        if runtime.fixed_storage_pair_bytes() != Some(self.declared_pair_bytes()) {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        Ok(Box::new(runtime))
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 0 }
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.config.latency())
    }
}
struct History {
    controls: crate::parameter_binding::Controls<Settings, { automation::COUNT }>,
    parameters: Parameters,
    first: Stage,
    final_stage: Option<Stage>,
    meter: DynamicsMeter,
    gain: f64,
    output: f64,
}
impl History {
    fn new(
        config: Config,
        admitted: [bool; automation::COUNT],
    ) -> Result<Self, DeviceContractError> {
        Self::with_capacity(config, admitted, config.delay + 1, config.mid_side)
    }
    /// Allocate bounded backing independently of the initial logical window.
    /// An unused final stage may be reserved; it stays silent until its owning
    /// layout is selected after a reset. No processing operation resizes storage.
    fn with_capacity(
        config: Config,
        admitted: [bool; automation::COUNT],
        capacity: usize,
        reserve_final_stage: bool,
    ) -> Result<Self, DeviceContractError> {
        if !(8000..=192000).contains(&config.rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let maximum = usize::try_from((u64::from(config.rate) * 6).div_ceil(1000))
            .map_err(|_| DeviceContractError::InvalidParameterValue)?
            .checked_add(TAPS)
            .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
        if capacity < config.delay + 1
            || capacity > maximum
            || (config.mid_side && !reserve_final_stage)
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(Self {
            controls: crate::parameter_binding::Controls::new(config.settings, admitted),
            parameters: config.parameters,
            first: Stage::new(capacity)?,
            final_stage: if reserve_final_stage {
                Some(Stage::new(capacity)?)
            } else {
                None
            },
            meter: DynamicsMeter::default(),
            gain: config.parameters.gain,
            output: config.parameters.output,
        })
    }
    fn same_storage_layout(&self, previous: &Self) -> bool {
        self.first.same_storage_layout(&previous.first)
            && match (&self.final_stage, &previous.final_stage) {
                (Some(a), Some(b)) => a.same_storage_layout(b),
                (None, None) => true,
                _ => false,
            }
    }
    fn begin_block(&mut self) {
        self.first.begin_block();
        if let Some(stage) = &mut self.final_stage {
            stage.begin_block();
        }
        self.meter = DynamicsMeter::default();
    }
    fn commit_from(&mut self, scratch: &mut Self, window: usize) {
        self.controls = scratch.controls;
        self.parameters = scratch.parameters;
        let first_dirty = scratch.first.dirty;
        self.first.sync_from(&scratch.first, &first_dirty, window);
        scratch.first.dirty = StageDirty::default();
        if let (Some(target), Some(source)) = (&mut self.final_stage, &mut scratch.final_stage) {
            let final_dirty = source.dirty;
            target.sync_from(source, &final_dirty, window);
            source.dirty = StageDirty::default();
        }
        self.meter = scratch.meter;
        self.gain = scratch.gain;
        self.output = scratch.output;
    }
    fn rollback_from(&mut self, committed: &Self, window: usize) {
        self.controls = committed.controls;
        self.parameters = committed.parameters;
        let first_dirty = self.first.dirty;
        self.first.sync_from(&committed.first, &first_dirty, window);
        if let (Some(target), Some(source)) = (&mut self.final_stage, &committed.final_stage) {
            let final_dirty = target.dirty;
            target.sync_from(source, &final_dirty, window);
        }
        self.meter = committed.meter;
        self.gain = committed.gain;
        self.output = committed.output;
    }
    fn copy_from(&mut self, old: &Self) {
        self.controls = old.controls;
        self.parameters = old.parameters;
        self.first.copy_from(&old.first);
        if let (Some(target), Some(source)) = (&mut self.final_stage, &old.final_stage) {
            target.copy_from(source);
        }
        self.meter = old.meter;
        self.gain = old.gain;
        self.output = old.output;
    }
    fn quiet(&self) -> bool {
        self.first.quiet() && self.final_stage.as_ref().is_none_or(Stage::quiet)
    }
    fn clear_attack(&mut self) {
        self.first.scalar.attack_step = [0.0; 2];
        if let Some(stage) = &mut self.final_stage {
            stage.scalar.attack_step = [0.0; 2];
        }
    }
    fn reset(&mut self, config: Config) {
        self.first.reset();
        if let Some(stage) = &mut self.final_stage {
            stage.reset();
        }
        self.meter = DynamicsMeter::default();
        self.controls.reset();
        self.parameters = config.parameters;
        self.gain = config.parameters.gain;
        self.output = config.parameters.output;
    }
    fn bytes(&self) -> usize {
        self.first.bytes() + self.final_stage.as_ref().map_or(0, Stage::bytes)
    }
}
/// Original fixed-layout runtime; fields and history custody remain private.
pub struct Runtime {
    config: Config,
    primary: History,
    scratch: History,
    open: bool,
    releasing: bool,
}

#[cfg(test)]
mod storage_tests;

#[cfg(test)]
mod detector_oracle_tests {
    use super::*;

    fn exhaustive_32_phase(x: &[f64; TAPS], phases: &[[f64; TAPS]; PHASES]) -> f64 {
        let mut max_val = 0.0_f64;
        for phase in phases {
            let mut acc = 0.0_f64;
            for (sample, coeff) in x.iter().zip(phase.iter()) {
                acc += sample * coeff;
            }
            max_val = max_val.max(acc.abs());
        }
        max_val
    }

    #[test]
    fn coarse_to_fine_true_peak_matches_exhaustive_32_phase_oracle() {
        let bank = reconstruction_bank();
        let phases = bank.phases_f64();
        let mut x = [0.0_f64; TAPS];
        let mut max_under_read = 0.0_f64;
        let mut state = 0x1234_5678_9abc_def0_u64;
        for step in 0..8_192usize {
            let sample = match step / 2048 {
                0 => {
                    // Near-Nyquist gated bursts
                    let env = if step % 97 < 13 { 10.0 } else { 0.5 };
                    env * (step as f64 * 2.71 + 0.3).sin()
                }
                1 => {
                    // Full-band frequency sweep
                    let t = (step % 2048) as f64 / 2048.0;
                    (std::f64::consts::PI * t * (step % 2048) as f64).sin()
                }
                2 => {
                    // Sparse impulses & step edges
                    if step % 37 == 0 {
                        2.5
                    } else if step % 37 == 1 {
                        -2.5
                    } else {
                        0.0
                    }
                }
                _ => {
                    // Deterministic white noise
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    ((state >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
                }
            };
            x.rotate_left(1);
            x[TAPS - 1] = sample;
            let exact = exhaustive_32_phase(&x, phases);
            let fast = bank.true_peak_lane_f64(&x);
            // `true_peak_lane` also inspects `x[16]` (phase 32 = next sample's phase 0),
            // so it is always >= exact (within floating-point rounding) and never under-reads.
            let under_read = (exact - fast).max(0.0);
            max_under_read = max_under_read.max(under_read);
            assert!(
                under_read <= 1e-6 * exact.max(1.0),
                "step={step}: fast={fast} exact={exact} under_read={under_read}"
            );
        }
        assert!(max_under_read <= 1e-6, "max_under_read={max_under_read}");
    }

    #[test]
    fn auto_release_taylor_matches_exp_across_rates_and_program_range() {
        for rate in [8_000u32, 44_100, 48_000, 96_000, 192_000] {
            let inv_rate = 1.0 / f64::from(rate);
            for step in 0..=100 {
                let program = step as f64 / 100.0;
                let seconds = 0.05 + 0.22 * program;
                let exact = 1.0 - (-1.0 / (seconds * f64::from(rate))).exp();
                let x = inv_rate / seconds;
                let approx = x * (1.0 - x * (0.5 - x * ((1.0 / 6.0) - x * (1.0 / 24.0))));
                assert!(
                    (approx - exact).abs() < 1e-14,
                    "rate={rate} program={program}: approx={approx} exact={exact}"
                );
            }
        }
    }
}

pub(super) mod structural;
