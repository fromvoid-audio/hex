//! Bounded owned rings with O(written interval) commit/rollback and O(1) reset.
//! A live retiring-donor transfer exchanges preallocated buffers; ordinary copy
//! inheritance remains available for independent control/offline oracles.
//! Musical delays use exact host local tempo, falling back to the last observed
//! tempo (initially 120 bpm) only when the borrowed clock is unavailable. Sync
//! times are bounded to 16 seconds; free time remains 1 ms–5 s. No callback work,
//! storage or tail duration grows with project duration. Repitch is a 10 ms
//! one-pole (20 ms with high quality); Fade crossfades taps over 50 ms, Jump
//! is immediate. Feedback, mix, filter enable/frequency/width ramp over 10 ms
//! on compatible edits. Modulation scales time by 1 + depth * oscillator and
//! filter center by 2^(4 * depth * oscillator); both are range-clamped. Filter
//! coefficients update every 16 frames independent of block partitions.
//! Ping-pong averages stereo input into the left line and crossfeeds outputs.
//! Linear and equal-power mix laws preserve exact dry/wet endpoints. Freeze is deliberately
//! unbounded until released/Stop/seek; it suppresses input, bypasses loop filtering
//! and uses integer taps to preserve a cycle without interpolation decay.
use super::*;
use super::{filter::BandPass, lfo::Lfo};
pub(super) const MAX_SECONDS: usize = 16;
pub(super) struct Factory {
    pub(super) settings: Settings,
    pub(super) rate: u32,
    pub(super) admitted: [bool; automation::COUNT],
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

    fn supports_smoothed_bypass(&self) -> bool {
        true
    }
    fn runtime_version(&self) -> u32 {
        2
    }
    fn declared_pair_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
            + (self.rate as usize * MAX_SECONDS + 4) * 2 * std::mem::size_of::<StereoFrame>()
    }
    fn create_runtime(&self) -> Result<Box<dyn OwnedProcessorRuntime>, DeviceContractError> {
        let length = self.rate as usize * MAX_SECONDS + 4;
        let ring = || {
            let mut v = Vec::new();
            v.try_reserve_exact(length)
                .map_err(|_| DeviceContractError::RuntimeAllocation)?;
            v.resize(length, [0.0; 2]);
            Ok(v)
        };
        let scalar = Scalar::new(self.settings, self.rate, self.admitted);
        let runtime = Runtime {
            settings: self.settings,
            rate: self.rate,
            committed: ring()?,
            scratch: ring()?,
            primary: scalar,
            speculative: scalar,
            dirty_start: 0,
            dirty_len: 0,
            open: false,
            releasing: false,
            pristine: true,
        };
        if runtime.fixed_storage_pair_bytes() != Some(self.declared_pair_bytes()) {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        Ok(Box::new(runtime))
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Unbounded
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
}
#[derive(Clone, Copy, Debug)]
struct Scalar {
    controls: crate::parameter_binding::Controls<automation::ControlValues, { automation::COUNT }>,
    next: usize,
    valid: usize,
    nonzero: usize,
    timing: automation::TimingControls,
    times: [f64; 2],
    old_times: [f64; 2],
    fade_left: u32,
    feedback: f64,
    mix: f64,
    dry_gain: f64,
    wet_gain: f64,
    filter_mix: f64,
    frequency: f64,
    width: f64,
    ramp_left: u32,
    tempo: u32,
    filters: [BandPass; 2],
    filter_tick: u32,
    lfo: Lfo,
}
impl Scalar {
    fn new(s: Settings, rate: u32, admitted: [bool; automation::COUNT]) -> Self {
        let timing = automation::TimingControls::new(s);
        let times = timing_times(timing, rate, 120_000_000);
        let mix = f64::from(s.value(DRY_WET));
        let (dry_gain, wet_gain) = mix_gains(mix, s.flag(EQUAL_LOUDNESS));
        let mut filters = [BandPass::default(); 2];
        for f in &mut filters {
            f.tune(
                f64::from(s.value(FREQUENCY)),
                f64::from(s.value(WIDTH)),
                rate,
            );
        }
        Self {
            controls: crate::parameter_binding::Controls::new(
                automation::ControlValues::new(s),
                admitted,
            ),
            next: 0,
            valid: 0,
            nonzero: 0,
            timing,
            times,
            old_times: times,
            fade_left: 0,
            feedback: f64::from(s.value(FEEDBACK)),
            mix,
            dry_gain,
            wet_gain,
            filter_mix: f64::from(s.flag(FILTER)),
            frequency: f64::from(s.value(FREQUENCY)),
            width: f64::from(s.value(WIDTH)),
            ramp_left: 0,
            tempo: 120_000_000,
            filters,
            filter_tick: 0,
            lfo: Lfo::default(),
        }
    }
    fn quiet(self, freeze: bool) -> bool {
        self.nonzero == 0 && (freeze || self.filters.iter().all(|f| f.quiet()))
    }
}
fn mix_gains(mix: f64, equal_loudness: bool) -> (f64, f64) {
    if equal_loudness {
        let (wet, dry) = (mix * std::f64::consts::FRAC_PI_2).sin_cos();
        (dry, wet)
    } else {
        (1.0 - mix, mix)
    }
}
fn timing_times(s: automation::TimingControls, rate: u32, tempo: u32) -> [f64; 2] {
    let divisions = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 16.0];
    std::array::from_fn(|i| {
        let seconds = if s.sync[i] {
            60_000_000.0 / f64::from(tempo.max(1)) * divisions[s.division[i] as usize] / 4.0
                * (1.0 + f64::from(s.offset[i]) / 100.0)
        } else {
            f64::from(s.time[i]) / 1000.0
        };
        (seconds * f64::from(rate)).clamp(
            f64::from(rate) / 1000.0,
            f64::from(rate) * MAX_SECONDS as f64,
        )
    })
}
fn lfo_increment(s: automation::LfoControls, rate: u32, tempo: u32) -> f64 {
    let mode = s.mode;
    let hz = match mode {
        0 => f64::from(s.rate),
        1 => 1000.0 / f64::from(s.time),
        _ => {
            let quarters = [
                0.0625, 0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0,
            ][s.division as usize];
            let period = quarters
                * match mode {
                    3 => 2.0 / 3.0,
                    4 => 1.5,
                    5 => 0.25,
                    _ => 1.0,
                };
            f64::from(tempo) / 60_000_000.0 / period
        }
    };
    hz / f64::from(rate)
}
pub(super) struct Runtime {
    settings: Settings,
    rate: u32,
    committed: Vec<StereoFrame>,
    scratch: Vec<StereoFrame>,
    primary: Scalar,
    speculative: Scalar,
    dirty_start: usize,
    dirty_len: usize,
    open: bool,
    releasing: bool,
    // Reset intentionally leaves old bytes in the rings; only constructor-owned
    // rings are known to be empty without a callback-time scan.
    pristine: bool,
}
fn finite(v: f64) -> f32 {
    v.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
}
fn sample(ring: &[StereoFrame], s: &Scalar, age: usize, lane: usize) -> f64 {
    if age == 0 || age > s.valid {
        return 0.0;
    }
    f64::from(ring[(s.next + ring.len() - age) % ring.len()][lane])
}
fn tap(ring: &[StereoFrame], s: &Scalar, time: f64, lane: usize, hq: bool, freeze: bool) -> f64 {
    if freeze {
        return sample(ring, s, time.round() as usize, lane);
    }
    let whole = time.floor() as usize;
    let f = time - whole as f64;
    let a = sample(ring, s, whole, lane);
    let b = sample(ring, s, whole + 1, lane);
    if !hq || f == 0.0 {
        return a + (b - a) * f;
    }
    let previous = sample(ring, s, whole - 1, lane);
    let next = sample(ring, s, whole + 2, lane);
    a + 0.5
        * f
        * (b - previous
            + f * (2.0 * previous - 5.0 * a + 4.0 * b - next
                + f * (3.0 * (a - b) + next - previous)))
}
impl Runtime {
    fn copy_interval(
        source: &[StereoFrame],
        destination: &mut [StereoFrame],
        start: usize,
        length: usize,
    ) {
        let first = length.min(source.len() - start);
        destination[start..start + first].copy_from_slice(&source[start..start + first]);
        destination[..length - first].copy_from_slice(&source[..length - first]);
    }
    fn inherit_scalars(&mut self, old: Scalar, old_settings: Settings) {
        self.pristine = false;
        let mut controls = self.primary.controls;
        controls.inherit(&old.controls);
        self.primary = old;
        self.primary.controls = controls;
        (self.primary.dry_gain, self.primary.wet_gain) =
            mix_gains(self.primary.mix, self.settings.flag(EQUAL_LOUDNESS));
        // Bound targets are installed by the next borrowed sample/fallback.
        // Only unbound canonical controls retime this shared ramp immediately.
        let mut unbound = old_settings;
        for binding in &automation::BINDINGS {
            let id = binding.descriptor.id;
            unbound.set(id, self.settings.value(id));
        }
        if self.settings != unbound {
            self.primary.ramp_left = self.rate.div_ceil(100);
        }
        self.speculative = self.primary;
        self.dirty_start = self.primary.next;
        self.dirty_len = 0;
        self.open = false;
    }
    fn block_settings(&self) -> BlockSettings {
        let hq = self.settings.flag(HIGH_QUALITY);
        BlockSettings {
            freeze: self.settings.flag(FREEZE) && !self.releasing,
            hq,
            alpha: 1.0 - (-1.0 / (f64::from(self.rate) * if hq { 0.02 } else { 0.01 })).exp(),
            fade_frames: self.rate.div_ceil(20),
        }
    }
    fn restore_controls(&mut self) {
        let old = self.speculative.controls.effective().other_targets();
        let old_timing = self.speculative.timing;
        self.speculative.controls.reset();
        self.speculative.timing = automation::timing(&self.speculative.controls);
        if old != self.speculative.controls.effective().other_targets()
            || old_timing != self.speculative.timing
        {
            self.speculative.ramp_left = self.rate.div_ceil(100);
        }
    }
    fn process(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        bypassed: bool,
    ) {
        if !self.open {
            self.begin_block();
        }
        if !frames.is_empty() && !self.releasing {
            self.restore_controls();
        }
        let block = self.block_settings();
        for (offset, frame) in frames.iter_mut().enumerate() {
            self.process_frame(
                frame,
                clock.and_then(|c| c.tempo_micro_bpm_at_offset(offset)),
                bypassed,
                block,
            );
        }
    }
    fn process_automation(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        values: &dyn hex_lattice::NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        if !self.open {
            self.begin_block();
        }
        let mut resolved = self
            .speculative
            .controls
            .resolve(&automation::BINDINGS, values)?;
        let block = self.block_settings();
        let timing_admitted = [0, 3, 4, 5, 6, 7, 8, 9, 10]
            .iter()
            .any(|&i| self.speculative.controls.admitted(i));
        // An incoming factory without timing lanes may still inherit a donor's
        // targets. Refresh once after its first installed fallback; thereafter
        // no borrowed value can change timing when it was not admitted.
        let mut refresh_timing = true;
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) = self.speculative.controls.sample(
                &automation::BINDINGS,
                &mut resolved,
                values,
                offset,
            )? {
                let changed = next.effective().other_targets()
                    != self.speculative.controls.effective().other_targets();
                self.speculative.controls.install(next);
                let mut retime = changed;
                if refresh_timing {
                    let timing = automation::timing(&self.speculative.controls);
                    retime |= timing != self.speculative.timing;
                    self.speculative.timing = timing;
                    refresh_timing = timing_admitted;
                }
                if retime {
                    self.speculative.ramp_left = self.rate.div_ceil(100);
                }
            }
            self.process_frame(
                frame,
                clock.and_then(|c| c.tempo_micro_bpm_at_offset(offset)),
                bypassed,
                block,
            );
        }
        Ok(())
    }
    // Preserve block-loop optimization: an out-of-line call here regresses
    // canonical processing. The measured old/new kernel comparison covers it.
    #[inline(always)]
    fn process_frame(
        &mut self,
        frame: &mut StereoFrame,
        tempo: Option<u32>,
        bypassed: bool,
        block: BlockSettings,
    ) {
        let settings = self.settings;
        let rate = self.rate;
        let BlockSettings {
            freeze,
            hq,
            alpha,
            fade_frames,
        } = block;
        let s = &mut self.speculative;
        let targets = s.controls.effective();
        if let Some(tempo) = tempo.filter(|v| *v > 0) {
            s.tempo = tempo;
        }
        let oscillator = s.lfo.process(
            targets.lfo.wave,
            f64::from(targets.lfo.morph),
            lfo_increment(targets.lfo, rate, s.tempo),
        );
        let mut requested = timing_times(s.timing, rate, s.tempo);
        for time in &mut requested {
            *time = (*time * (1.0 + oscillator * f64::from(targets.lfo.delay))).clamp(
                f64::from(rate) / 1000.0,
                f64::from(rate) * MAX_SECONDS as f64,
            );
        }
        if s.valid == 0 {
            s.times = requested;
            s.old_times = requested;
        }
        match settings.choice(SMOOTHING) {
            0 => {
                for (time, target) in s.times.iter_mut().zip(requested) {
                    *time += alpha * (target - *time);
                    if (target - *time).abs() < 1e-8 {
                        *time = target;
                    }
                }
                s.fade_left = 0;
            }
            1 => {
                if s.fade_left == 0
                    && s.times
                        .iter()
                        .zip(requested)
                        .any(|(a, b)| (a - b).abs() > 1e-8)
                {
                    s.old_times = s.times;
                    s.fade_left = fade_frames;
                }
                s.times = requested;
            }
            _ => {
                s.times = requested;
                s.fade_left = 0;
            }
        }
        if s.ramp_left > 0 {
            s.feedback += (f64::from(targets.feedback) - s.feedback) / f64::from(s.ramp_left);
            s.mix += (f64::from(targets.mix) - s.mix) / f64::from(s.ramp_left);
            s.filter_mix +=
                (f64::from(settings.flag(FILTER)) - s.filter_mix) / f64::from(s.ramp_left);
            s.frequency += (f64::from(targets.frequency) - s.frequency) / f64::from(s.ramp_left);
            s.width += (f64::from(targets.width) - s.width) / f64::from(s.ramp_left);
            s.ramp_left -= 1;
            if s.ramp_left == 0 {
                s.feedback = f64::from(targets.feedback);
                s.mix = f64::from(targets.mix);
                s.filter_mix = f64::from(settings.flag(FILTER));
                s.frequency = f64::from(targets.frequency);
                s.width = f64::from(targets.width);
            }
            (s.dry_gain, s.wet_gain) = mix_gains(s.mix, settings.flag(EQUAL_LOUDNESS));
        }
        let scalar = &*s;
        let mut wet = std::array::from_fn::<_, 2, _>(|lane| {
            tap(&self.scratch, scalar, s.times[lane], lane, hq, freeze)
        });
        if s.fade_left > 0 && !freeze {
            let weight = 1.0 - f64::from(s.fade_left) / f64::from(fade_frames);
            for (lane, value) in wet.iter_mut().enumerate() {
                let old = tap(&self.scratch, scalar, s.old_times[lane], lane, hq, false);
                *value = old * (1.0 - weight) + *value * weight;
            }
            s.fade_left -= 1;
        }
        if s.filter_tick == 0 {
            let frequency = (s.frequency
                * 2.0_f64.powf(oscillator * f64::from(targets.lfo.filter) * 4.0))
            .clamp(20.0, 20000.0);
            for filter in &mut s.filters {
                filter.tune(frequency, s.width, rate);
            }
        }
        s.filter_tick = (s.filter_tick + 1) % 16;
        let dry = frame.map(|v| if v.is_finite() { f64::from(v) } else { 0.0 });
        let mut write = [0.0; 2];
        for lane in 0..2 {
            let feedback = if settings.flag(PING_PONG) {
                wet[1 - lane]
            } else {
                wet[lane]
            };
            let input = if settings.flag(PING_PONG) {
                if lane == 0 {
                    (dry[0] + dry[1]) * 0.5
                } else {
                    0.0
                }
            } else {
                dry[lane]
            };
            let loop_input = if freeze {
                feedback
            } else {
                input + feedback * s.feedback
            };
            let filtered = s.filters[lane].process(if freeze { 0.0 } else { loop_input });
            let value = finite(if freeze {
                loop_input
            } else {
                loop_input * (1.0 - s.filter_mix) + filtered * s.filter_mix
            });
            write[lane] = if value.abs() < 1e-12 { 0.0 } else { value };
        }
        if !bypassed {
            *frame =
                std::array::from_fn(|lane| finite(dry[lane] * s.dry_gain + wet[lane] * s.wet_gain));
        }
        if s.valid == self.scratch.len() {
            s.nonzero -= usize::from(self.scratch[s.next] != [0.0; 2]);
        } else {
            s.valid += 1;
        }
        s.nonzero += usize::from(write != [0.0; 2]);
        self.scratch[s.next] = write;
        s.next += 1;
        if s.next == self.scratch.len() {
            s.next = 0;
        }
        self.dirty_len = self.dirty_len.saturating_add(1).min(self.scratch.len());
    }
}
#[derive(Clone, Copy)]
struct BlockSettings {
    freeze: bool,
    hq: bool,
    alpha: f64,
    fade_frames: u32,
}
impl OwnedProcessorRuntime for Runtime {
    fn supports_parameter_automation(&self, id: hex_lattice_contract::ParameterId) -> bool {
        self.primary.controls.supports(&automation::BINDINGS, id)
    }
    fn process_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        values: &dyn hex_lattice::NativeParameterAutomation,
        _: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.process_automation(frames, clock, values, false)
    }
    fn process_bypassed_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        values: &dyn hex_lattice::NativeParameterAutomation,
        _: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.process_automation(frames, clock, values, true)
    }

    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>().checked_add(
            self.committed
                .capacity()
                .checked_add(self.scratch.capacity())?
                .checked_mul(std::mem::size_of::<StereoFrame>())?,
        )
    }
    fn begin_block(&mut self) {
        if self.open {
            self.rollback_block();
        }
        self.dirty_start = self.speculative.next;
        self.dirty_len = 0;
        self.open = true;
    }
    fn process_block(&mut self, frames: &mut [StereoFrame], _: &dyn hex_lattice::DeviceResources) {
        self.process(frames, None, false);
    }
    fn process_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        _: &dyn hex_lattice::DeviceResources,
    ) {
        self.process(frames, clock, false);
    }
    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        _: &dyn hex_lattice::DeviceResources,
    ) {
        self.process(frames, clock, true);
    }
    fn commit_block(&mut self) {
        if !self.open {
            return;
        }
        Self::copy_interval(
            &self.scratch,
            &mut self.committed,
            self.dirty_start,
            self.dirty_len,
        );
        self.primary = self.speculative;
        self.dirty_len = 0;
        self.open = false;
    }
    fn rollback_block(&mut self) {
        if !self.open {
            return;
        }
        Self::copy_interval(
            &self.committed,
            &mut self.scratch,
            self.dirty_start,
            self.dirty_len,
        );
        self.speculative = self.primary;
        self.dirty_len = 0;
        self.open = false;
    }
    fn begin_output_tail(&mut self) {
        self.pristine = false;
        self.releasing = true;
    }
    fn reset(&mut self) {
        self.pristine = false;
        self.releasing = false;
        let admitted = std::array::from_fn(|i| self.primary.controls.admitted(i));
        let s = Scalar::new(self.settings, self.rate, admitted);
        self.primary = s;
        self.speculative = s;
        self.dirty_start = 0;
        self.dirty_len = 0;
        self.open = false;
    }
    fn committed_is_quiescent(&self) -> bool {
        self.primary
            .quiet(self.settings.flag(FREEZE) && !self.releasing)
    }
    fn speculative_is_quiescent(&self) -> bool {
        self.speculative
            .quiet(self.settings.flag(FREEZE) && !self.releasing)
    }
    fn can_inherit_from(&self, old: &dyn OwnedProcessorRuntime) -> bool {
        old.as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| !self.open && !old.open && self.rate == old.rate)
    }
    fn inherit_from(&mut self, old: &dyn OwnedProcessorRuntime) {
        if !self.can_inherit_from(old) {
            return;
        }
        if let Some(old) = old.as_any().downcast_ref::<Self>() {
            let start =
                (old.primary.next + old.committed.len() - old.primary.valid) % old.committed.len();
            Self::copy_interval(
                &old.committed,
                &mut self.committed,
                start,
                old.primary.valid,
            );
            Self::copy_interval(&old.committed, &mut self.scratch, start, old.primary.valid);
            self.inherit_scalars(old.primary, old.settings);
        }
    }
    fn transfer_history_from(&mut self, old: &mut dyn OwnedProcessorRuntime) {
        if !self.can_inherit_from(old) {
            return;
        }
        if let Some(old) = old.as_any_mut().and_then(|v| v.downcast_mut::<Self>()) {
            std::mem::swap(&mut self.committed, &mut old.committed);
            std::mem::swap(&mut self.scratch, &mut old.scratch);
            self.inherit_scalars(old.primary, old.settings);
            old.reset();
        }
    }
    fn can_fork_timing_without_signal_history_from(
        &self,
        previous: &dyn OwnedProcessorRuntime,
    ) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|old| {
            self.pristine
                && self.primary.valid == 0
                && self.speculative.valid == 0
                && !self.open
                && !old.open
                && !old.releasing
                && self.rate == old.rate
                && self.settings == old.settings
                && !self.settings.flag(FREEZE)
                && self.committed.len() == old.committed.len()
                && self.scratch.len() == old.scratch.len()
        })
    }
    fn fork_timing_without_signal_history_from(
        &mut self,
        previous: &dyn OwnedProcessorRuntime,
    ) -> bool {
        if !self.can_fork_timing_without_signal_history_from(previous) {
            return false;
        }
        let Some(old) = previous.as_any().downcast_ref::<Self>() else {
            return false;
        };
        let mut controls = self.primary.controls;
        controls.inherit(&old.primary.controls);
        let mut scalar = old.primary;
        scalar.controls = controls;
        scalar.nonzero = 0;
        scalar.filters = scalar.filters.map(BandPass::without_signal_memory);
        self.primary = scalar;
        self.speculative = scalar;
        self.dirty_start = scalar.next;
        self.dirty_len = 0;
        self.pristine = false;
        true
    }
    fn can_split_tail_input_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        self.can_fork_timing_without_signal_history_from(previous)
    }
    fn can_retarget_parameters_from(&self, old: &dyn OwnedProcessorRuntime) -> bool {
        self.can_inherit_from(old)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}
