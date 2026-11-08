//! Owned lookahead transitions at one immutable, control-declared latency.
//!
//! Three configurations are prepared on control for fixed mode/routing. Only
//! two branches render: the current branch and a reset successor. The successor
//! warms for the declared latency, then receives one 256-frame linear fade.
//! Gain-ramp positions survive its reset; detector/release history starts fresh.
//! A single latest pending choice waits until completion, bounding reset work
//! to at most once per warmup+fade, even with a choice on every input frame.
//!
//! Both histories include controls, selectors, padding and output telemetry.
//! Only live branches need checkpoint copies: an inactive branch is reset
//! before reuse. Rollback never changes committed storage. Padding carries
//! gain-applied input before the limiting stages. Ceiling/output changes remain
//! effective at the current output frame; blends report weighted attenuation
//! and clipping from contributing branches. This is an original transition law,
//! not seamless historical-state reconstruction or a Live-parity claim.
use super::super::{
    decode,
    state::{Settings, parameter::*},
};
use super::automation as scalar;
use super::{BlockStats, Config, History, Parameters, Peak, Runtime as PlainRuntime, Sample};
use crate::parameter_binding::{Binding, CHOICE_ENVELOPE, Controls};
use hex_lattice::{
    DynamicsMeter, NativeParameterAutomation, OutputTail, OwnedProcessorRuntime,
    PreparedOwnedProcessorRuntime, StereoFrame,
};
use hex_lattice_contract::{
    DeviceContractError as Error, NativeDeviceState, ParameterChangeImpact, ParameterDescriptor,
    ParameterId, ParameterSemantics, ParameterSmoothing,
};
use std::{any::Any, mem::size_of};
const COUNT: usize = 10;
const FADE: u32 = 256;
const BINDINGS: [Binding<Settings>; COUNT] = {
    let [
        gain,
        ceiling,
        release,
        auto,
        link,
        maximize,
        threshold,
        output,
        ms_link,
    ] = scalar::BINDINGS;
    [
        gain,
        ceiling,
        release,
        auto,
        link,
        Binding::choice(
            super::super::state::DESCRIPTORS[5],
            ParameterSemantics {
                smoothing: ParameterSmoothing::DeviceManaged,
                change_impact: ParameterChangeImpact::Structural,
                ..CHOICE_ENVELOPE
            },
            |s| s.choice(LOOKAHEAD),
            |s, v| {
                if v > 2 {
                    return Err(Error::InvalidParameterValue);
                }
                s.0[5] = v as f32;
                Ok(())
            },
        ),
        maximize,
        threshold,
        output,
        ms_link,
    ]
};
pub(in crate::limiter) fn semantics() -> ParameterSemantics {
    BINDINGS[5].semantics
}
fn slot(id: ParameterId) -> Option<usize> {
    BINDINGS.binary_search_by_key(&id, |b| b.descriptor.id).ok()
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Look {
    Short,
    Medium,
    Long,
}
impl Look {
    fn new(value: u32) -> Result<Self, Error> {
        match value {
            0 => Ok(Self::Short),
            1 => Ok(Self::Medium),
            2 => Ok(Self::Long),
            _ => Err(Error::InvalidParameterValue),
        }
    }
    fn index(self) -> usize {
        match self {
            Self::Short => 0,
            Self::Medium => 1,
            Self::Long => 2,
        }
    }
}
/// Control-prepared ten-control factory. Each created owner retains its own
/// immutable configurations and both histories, independent of factory life.
/// This constructor alone grants no host metadata/publication capability.
#[derive(Clone, Copy)]
pub struct PreparedLookahead {
    settings: Settings,
    configurations: [Config; 3],
    padding: [usize; 3],
    base: Look,
    admitted: [bool; COUNT],
    capacity: usize,
    pad_capacity: usize,
    latency: u32,
    bytes: usize,
}
impl PreparedLookahead {
    pub(in crate::limiter) fn new(
        state: &NativeDeviceState,
        rate: u32,
        ids: &[ParameterId],
    ) -> Result<Self, Error> {
        state.require_asset_free_processing()?;
        if !(8000..=192000).contains(&rate)
            || ids.is_empty()
            || ids.windows(2).any(|w| w[0] >= w[1])
        {
            return Err(Error::InvalidParameterValue);
        }
        let settings = decode(state)?;
        let mut admitted = [false; COUNT];
        for &id in ids {
            admitted[slot(id).ok_or(Error::UnsupportedProcessingClass)?] = true;
        }
        if !admitted[5] {
            return Err(Error::UnsupportedProcessingClass);
        }
        let base = Look::new(settings.choice(LOOKAHEAD))?;
        let configurations: [Config; 3] = std::array::from_fn(|look| {
            let mut s = settings;
            s.0[5] = look as f32;
            Config::new(s, rate)
        });
        let latency = configurations[2].latency();
        if latency == 0 || latency > hex_lattice::MAX_RESERVED_BYPASS_FRAMES {
            return Err(Error::RuntimeBudgetExceeded);
        }
        let mut padding = [0; 3];
        for (pad, config) in padding.iter_mut().zip(configurations) {
            *pad = usize::try_from(
                latency
                    .checked_sub(config.latency())
                    .ok_or(Error::RuntimeBudgetExceeded)?,
            )
            .map_err(|_| Error::RuntimeBudgetExceeded)?;
        }
        let capacity = configurations[2].delay + 1;
        let pad_capacity = padding[0]
            .checked_add(1)
            .ok_or(Error::RuntimeBudgetExceeded)?;
        let history_bytes = capacity
            .checked_mul(size_of::<Sample>() + 2 * size_of::<Peak>())
            .and_then(|n| n.checked_mul(if configurations[0].mid_side { 2 } else { 1 }))
            .ok_or(Error::RuntimeBudgetExceeded)?;
        let bytes = pad_capacity
            .checked_mul(size_of::<[f64; 2]>())
            .and_then(|n| n.checked_add(history_bytes))
            .and_then(|n| n.checked_mul(4))
            .and_then(|n| n.checked_add(size_of::<Runtime>()))
            .ok_or(Error::RuntimeBudgetExceeded)?;
        Ok(Self {
            settings,
            configurations,
            padding,
            base,
            admitted,
            capacity,
            pad_capacity,
            latency,
            bytes,
        })
    }
    fn config(&self, look: Look) -> &Config {
        &self.configurations[look.index()]
    }
}
impl PreparedOwnedProcessorRuntime for PreparedLookahead {
    fn reserved_bypass_latency_frames(&self) -> Option<u32> {
        Some(self.latency)
    }
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        slot(id).is_some_and(|i| self.admitted[i])
    }
    fn parameter_automation_descriptor(&self, id: ParameterId) -> Option<ParameterDescriptor> {
        slot(id)
            .filter(|&i| self.admitted[i])
            .map(|i| BINDINGS[i].descriptor)
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_pair_bytes(&self) -> usize {
        self.bytes
    }
    fn create_runtime(&self) -> Result<Box<dyn OwnedProcessorRuntime>, Error> {
        let runtime = Runtime {
            prepared: *self,
            primary: State::new(self)?,
            scratch: State::new(self)?,
            open: false,
            releasing: false,
        };
        if runtime.fixed_storage_pair_bytes() != Some(self.bytes) {
            return Err(Error::RuntimeBudgetExceeded);
        }
        Ok(Box::new(runtime))
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 0 }
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.latency)
    }
}
#[derive(Clone, Copy)]
struct Record {
    frame: StereoFrame,
    attenuation: [f64; 2],
    clipped: bool,
}
struct Padding {
    records: Vec<[f64; 2]>,
    next: usize,
    nonzero: usize,
}
impl Padding {
    fn new(capacity: usize) -> Result<Self, Error> {
        Ok(Self {
            records: super::allocate(capacity)?,
            next: 0,
            nonzero: 0,
        })
    }
    fn reset(&mut self) {
        self.records.fill([0.0; 2]);
        self.next = 0;
        self.nonzero = 0;
    }
    fn copy_from(&mut self, old: &Self) {
        self.records.copy_from_slice(&old.records);
        self.next = old.next;
        self.nonzero = old.nonzero;
    }
    fn process(&mut self, record: [f64; 2], delay: usize) -> [f64; 2] {
        if delay == 0 {
            return record;
        }
        let old = self.records[self.next];
        self.nonzero -= usize::from(old != [0.0; 2]);
        self.nonzero += usize::from(record != [0.0; 2]);
        self.records[self.next] = record;
        self.next = (self.next + 1) % delay;
        old
    }
}
struct Branch {
    history: History,
    padding: Padding,
}
impl Branch {
    fn new(p: &PreparedLookahead) -> Result<Self, Error> {
        Ok(Self {
            history: History::with_capacity(
                *p.config(p.base),
                [false; scalar::COUNT],
                p.capacity,
                p.config(p.base).mid_side,
            )?,
            padding: Padding::new(p.pad_capacity)?,
        })
    }
    fn copy_from(&mut self, old: &Self) {
        self.history.copy_from(&old.history);
        self.padding.copy_from(&old.padding);
    }
    fn reset(
        &mut self,
        p: &PreparedLookahead,
        look: Look,
        settings: Settings,
        gain: f64,
        output: f64,
    ) {
        self.history.reset(*p.config(look));
        self.padding.reset();
        self.history.gain = gain;
        self.history.output = output;
        self.history.parameters = Parameters::new(settings, p.config(look).rate);
    }
    fn render(&mut self, p: &PreparedLookahead, look: Look, input: [f64; 2]) -> Record {
        let signal = self.padding.process(input, p.padding[look.index()]);
        let mut frame = [0.0; 2];
        let mut stats = BlockStats::new();
        self.history.meter.clipped = false;
        PlainRuntime::process_signal(
            p.config(look),
            &mut self.history,
            signal,
            &mut frame,
            &mut stats,
        );
        Record {
            frame,
            attenuation: stats.min_attenuation,
            clipped: self.history.meter.clipped,
        }
    }
    fn quiet(&self) -> bool {
        self.history.quiet() && self.padding.nonzero == 0
    }
    fn bytes(&self) -> Option<usize> {
        self.padding
            .records
            .capacity()
            .checked_mul(size_of::<[f64; 2]>())?
            .checked_add(self.history.bytes())
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Stable,
    Warming(u32),
    Fading(u32),
}
struct State {
    controls: Controls<Settings, COUNT>,
    branches: [Branch; 2],
    looks: [Look; 2],
    active: usize,
    phase: Phase,
    pending: Option<Look>,
    pristine: bool,
    meter: DynamicsMeter,
}
impl State {
    fn new(p: &PreparedLookahead) -> Result<Self, Error> {
        Ok(Self {
            controls: Controls::new(p.settings, p.admitted),
            branches: [Branch::new(p)?, Branch::new(p)?],
            looks: [p.base; 2],
            active: 0,
            phase: Phase::Stable,
            pending: None,
            pristine: true,
            meter: DynamicsMeter::default(),
        })
    }
    fn copy_from(&mut self, old: &Self) {
        self.controls = old.controls;
        self.looks = old.looks;
        self.active = old.active;
        self.phase = old.phase;
        self.pending = old.pending;
        self.pristine = old.pristine;
        self.meter = old.meter;
        self.branches[old.active].copy_from(&old.branches[old.active]);
        if old.phase != Phase::Stable {
            self.branches[1 - old.active].copy_from(&old.branches[1 - old.active]);
        }
    }
    fn start(&mut self, p: &PreparedLookahead, look: Look, settings: Settings) {
        let next = 1 - self.active;
        let gain = self.branches[self.active].history.gain;
        let output = self.branches[self.active].history.output;
        self.branches[next].reset(p, look, settings, gain, output);
        self.looks[next] = look;
        self.phase = Phase::Warming(p.latency);
    }
    fn targets(
        &mut self,
        p: &PreparedLookahead,
        settings: Settings,
        look: Look,
        scalars_changed: bool,
    ) {
        if scalars_changed {
            let parameters = Parameters::new(settings, p.config(look).rate);
            self.branches[self.active].history.parameters = parameters;
            if self.phase != Phase::Stable {
                self.branches[1 - self.active].history.parameters = parameters;
            }
        }
        if self.pristine {
            if look != self.looks[self.active] {
                let gain = self.branches[self.active].history.gain;
                let output = self.branches[self.active].history.output;
                self.branches[self.active].reset(p, look, settings, gain, output);
                self.looks[self.active] = look;
            }
        } else if self.phase == Phase::Stable {
            if look != self.looks[self.active] {
                self.start(p, look, settings);
            }
        } else {
            self.pending = (look != self.looks[1 - self.active]).then_some(look);
        }
    }
    fn frame(&mut self, p: &PreparedLookahead, frame: &mut StereoFrame, stats: &mut BlockStats) {
        self.pristine = false;
        let signal = PlainRuntime::prepare_input(
            p.config(self.looks[self.active]),
            &mut self.branches[self.active].history,
            *frame,
            stats,
        );
        let gain = self.branches[self.active].history.gain;
        let output = self.branches[self.active].history.output;
        let old = self.branches[self.active].render(p, self.looks[self.active], signal);
        let mut record = old;
        if self.phase != Phase::Stable {
            let next = 1 - self.active;
            self.branches[next].history.gain = gain;
            self.branches[next].history.output = output;
            let new = self.branches[next].render(p, self.looks[next], signal);
            match self.phase {
                Phase::Warming(left) => {
                    self.phase = if left == 1 {
                        Phase::Fading(0)
                    } else {
                        Phase::Warming(left - 1)
                    }
                }
                Phase::Fading(position) => {
                    let weight = (position + 1) as f32 / FADE as f32;
                    record.frame = std::array::from_fn(|i| {
                        old.frame[i] * (1.0 - weight) + new.frame[i] * weight
                    });
                    record.attenuation = std::array::from_fn(|i| {
                        old.attenuation[i] * (1.0 - f64::from(weight))
                            + new.attenuation[i] * f64::from(weight)
                    });
                    record.clipped = (weight < 1.0 && old.clipped) || new.clipped;
                    if position + 1 == FADE {
                        self.active = next;
                        self.phase = Phase::Stable;
                        if let Some(look) = self.pending.take()
                            && look != self.looks[next]
                        {
                            self.start(p, look, *self.controls.effective_ref());
                        }
                    } else {
                        self.phase = Phase::Fading(position + 1);
                    }
                }
                Phase::Stable => {}
            }
        }
        *frame = record.frame;
        self.meter.clipped |= record.clipped;
        for (minimum, attenuation) in stats.min_attenuation.iter_mut().zip(record.attenuation) {
            *minimum = minimum.min(attenuation);
        }
    }
    fn quiet(&self) -> bool {
        self.branches[self.active].quiet()
            && (self.phase == Phase::Stable || self.branches[1 - self.active].quiet())
    }
    fn reset(&mut self, p: &PreparedLookahead) {
        for branch in &mut self.branches {
            branch.reset(
                p,
                p.base,
                p.settings,
                p.config(p.base).parameters.gain,
                p.config(p.base).parameters.output,
            );
        }
        self.controls.reset();
        self.looks = [p.base; 2];
        self.active = 0;
        self.phase = Phase::Stable;
        self.pending = None;
        self.pristine = true;
        self.meter = DynamicsMeter::default();
    }
    fn bytes(&self) -> Option<usize> {
        self.branches[0]
            .bytes()?
            .checked_add(self.branches[1].bytes()?)
    }
}
fn scalar_change(a: &Settings, b: &Settings) -> bool {
    a.0[..5] != b.0[..5] || a.0[6..] != b.0[6..]
}
struct Runtime {
    prepared: PreparedLookahead,
    primary: State,
    scratch: State,
    open: bool,
    releasing: bool,
}
impl Runtime {
    fn plain(&mut self, frames: &mut [StereoFrame], bypassed: bool) {
        if !self.open {
            return;
        }
        if !frames.is_empty() && !self.releasing {
            let base = self.scratch.controls.base();
            let changed = scalar_change(&base, self.scratch.controls.effective_ref());
            self.scratch.controls.reset();
            self.scratch
                .targets(&self.prepared, base, self.prepared.base, changed);
        }
        let mut stats = BlockStats::new();
        for frame in frames {
            let dry = *frame;
            self.scratch.frame(&self.prepared, frame, &mut stats);
            if bypassed {
                *frame = dry;
            }
        }
        stats.finish_meter(&mut self.scratch.meter);
    }
    fn automated(
        &mut self,
        frames: &mut [StereoFrame],
        values: &dyn NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), Error> {
        if !self.open {
            self.begin_block();
        }
        let mut map = self.scratch.controls.resolve(&BINDINGS, values)?;
        let mut stats = BlockStats::new();
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) = self
                .scratch
                .controls
                .sample(&BINDINGS, &mut map, values, offset)?
            {
                let settings = *next.effective_ref();
                let look = Look::new(settings.choice(LOOKAHEAD))?;
                let changed = scalar_change(&settings, self.scratch.controls.effective_ref());
                self.scratch
                    .targets(&self.prepared, settings, look, changed);
                self.scratch.controls.install(next);
            }
            let dry = *frame;
            self.scratch.frame(&self.prepared, frame, &mut stats);
            if bypassed {
                *frame = dry;
            }
        }
        stats.finish_meter(&mut self.scratch.meter);
        Ok(())
    }
}

impl OwnedProcessorRuntime for Runtime {
    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        size_of::<Self>()
            .checked_add(self.primary.bytes()?)?
            .checked_add(self.scratch.bytes()?)
    }
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.primary.controls.supports(&BINDINGS, id)
    }
    fn begin_block(&mut self) {
        self.scratch.copy_from(&self.primary);
        self.scratch.meter = DynamicsMeter::default();
        self.open = true;
    }
    fn process_block(&mut self, frames: &mut [StereoFrame], _: &dyn hex_lattice::DeviceResources) {
        // Plain processing uses only the immutable validated base and typed
        // layout. No borrowed values exist here, so this path cannot refuse.
        self.plain(frames, false);
    }
    fn process_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        values: &dyn NativeParameterAutomation,
        _: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), Error> {
        self.automated(frames, values, false)
    }
    fn process_bypassed_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        values: &dyn NativeParameterAutomation,
        _: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), Error> {
        self.automated(frames, values, true)
    }
    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        _: &dyn hex_lattice::DeviceResources,
    ) {
        self.plain(frames, true);
    }
    fn commit_block(&mut self) {
        if self.open {
            std::mem::swap(&mut self.primary, &mut self.scratch);
            self.open = false;
        }
    }
    fn rollback_block(&mut self) {
        self.open = false;
    }
    fn begin_output_tail(&mut self) {
        self.releasing = true;
        self.primary.pending = None;
        self.scratch.pending = None;
    }
    fn reset(&mut self) {
        self.primary.reset(&self.prepared);
        self.scratch.reset(&self.prepared);
        self.open = false;
        self.releasing = false;
    }
    fn committed_is_quiescent(&self) -> bool {
        self.primary.quiet()
    }
    fn speculative_is_quiescent(&self) -> bool {
        if self.open {
            self.scratch.quiet()
        } else {
            self.primary.quiet()
        }
    }
    fn can_inherit_from(&self, old: &dyn OwnedProcessorRuntime) -> bool {
        old.as_any().downcast_ref::<Self>().is_some_and(|old| {
            !self.open
                && !old.open
                && self.prepared.latency == old.prepared.latency
                && self.prepared.capacity == old.prepared.capacity
                && self.prepared.pad_capacity == old.prepared.pad_capacity
                && self.prepared.settings.choice(MODE) == old.prepared.settings.choice(MODE)
                && self.prepared.settings.choice(ROUTING) == old.prepared.settings.choice(ROUTING)
                && self.prepared.config(self.prepared.base).rate
                    == old.prepared.config(old.prepared.base).rate
        })
    }
    fn inherit_from(&mut self, old: &dyn OwnedProcessorRuntime) {
        if !self.can_inherit_from(old) {
            return;
        }
        if let Some(old) = old.as_any().downcast_ref::<Self>() {
            let mut controls = self.primary.controls;
            controls.inherit(&old.primary.controls);
            self.primary.copy_from(&old.primary);
            self.primary.controls = controls;
            self.scratch.copy_from(&self.primary);
            self.releasing = false;
        }
    }
    fn can_retarget_parameters_from(&self, old: &dyn OwnedProcessorRuntime) -> bool {
        self.can_inherit_from(old)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn take_dynamics_meter(&mut self) -> Option<DynamicsMeter> {
        let meter = self.primary.meter;
        self.primary.meter = DynamicsMeter::default();
        Some(meter)
    }
}

#[cfg(test)]
mod tests;
