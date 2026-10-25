//! Original stereo feedback delay, free time only (no tempo-sync or crossfeed).
//!
//! Time is 1–1000 ms, feedback 0–0.85 and dry/wet 0–1. Fractional reads use
//! linear interpolation. Compatible parameter edits preserve both channels and
//! ramp time/feedback/mix over 10 ms; changing time intentionally glides pitch.
//! Bypass advances warm history while returning exact dry input. Reset clears
//! history; sample-rate replacement starts fresh. Ordinary loop wraps preserve
//! this free-running history through the host's existing discontinuity policy.
//!
//! Each prepared runtime owns two fixed one-second stereo rings. Transactions
//! synchronize only the circular interval written since begin, including wraps;
//! rollback restores that interval and all scalar state. History transfer copies
//! the committed ring into a new owner on the existing bounded host handoff.
//! All allocation/construction/destruction remains control-side. The declared
//! pair includes object and actual reserved capacities, bounded by lattice policy.
//!
//! Feedback below unity decays, with writes below 1e-12 flushed to exact zero.
//! Tail metadata conservatively promises no finite deadline for every possible
//! finite signal. Quiescence counts the entire ring (including currently dormant
//! taps); a silent block is never proof. Existing host retained-tail capacity and
//! caller-selected finite export ranges bound admission and offline work.
use hex_lattice::{
    NativeDeviceDefinition, OutputTail, OwnedProcessorRuntime, PreparedOwnedProcessorRuntime,
    PreparedProcessorFactory, StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::{any::Any, sync::Arc};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.delay");
pub const TIME: ParameterId = ParameterId::from_static(1);
pub const FEEDBACK: ParameterId = ParameterId::from_static(2);
pub const DRY_WET: ParameterId = ParameterId::from_static(3);
const DESCRIPTORS: &[ParameterDescriptor] = &[
    ParameterDescriptor {
        id: TIME,
        label: "time",
        kind: ParameterKind::Float {
            min: 1.0,
            max: 1000.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(250.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: FEEDBACK,
        label: "feedback",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 0.85,
            unit: None,
        },
        default: ParameterValue::Float(0.35),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: DRY_WET,
        label: "dry/wet",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.25),
        presentation: ParameterPresentation::Linear,
    },
];
#[derive(Clone, Copy, PartialEq)]
struct Settings([f32; 3]);
fn decode(state: &NativeDeviceState) -> Result<Settings, DeviceContractError> {
    if state.type_id() != TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != 1 {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    if state.payload().len() != 12 {
        return Err(DeviceContractError::MalformedState);
    }
    let mut values = [0.0; 3];
    for ((value, descriptor), bytes) in values
        .iter_mut()
        .zip(DESCRIPTORS)
        .zip(state.payload().chunks_exact(4))
    {
        *value = f32::from_le_bytes(
            bytes
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?,
        );
        descriptor.validate(ParameterValue::Float(*value))?;
    }
    Ok(Settings(values))
}
fn encode(settings: Settings) -> Result<NativeDeviceState, DeviceContractError> {
    let mut bytes = Vec::with_capacity(12);
    for (descriptor, value) in DESCRIPTORS.iter().zip(settings.0) {
        descriptor.validate(ParameterValue::Float(value))?;
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    NativeDeviceState::new(TYPE_ID, 1, bytes)
}
pub struct DelayDefinition;
pub static DEFINITION: DelayDefinition = DelayDefinition;
impl NativeDeviceMetadata for DelayDefinition {
    fn type_id(&self) -> DeviceTypeId {
        TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("delay")
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<hex_lattice_contract::ParameterSemantics>, DeviceContractError> {
        use hex_lattice_contract::{
            ParameterAccess, ParameterAutomation, ParameterChangeImpact, ParameterModulation,
            ParameterNormalization, ParameterSemantics, ParameterSmoothing, ParameterStepping,
        };
        decode(state)?;
        if !DESCRIPTORS.iter().any(|p| p.id == id) {
            return Err(DeviceContractError::UnknownParameter);
        }
        Ok(Some(ParameterSemantics {
            access: ParameterAccess::ReadWrite,
            normalization: if id == TIME {
                ParameterNormalization::PositiveLogarithmic
            } else {
                ParameterNormalization::Linear
            },
            stepping: ParameterStepping::Continuous,
            automation: ParameterAutomation::Unsupported,
            modulation: ParameterModulation::Unsupported,
            smoothing: ParameterSmoothing::DeviceManaged,
            change_impact: ParameterChangeImpact::ValueOnly,
        }))
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(Settings([250.0, 0.35, 0.25]))
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        encode(decode(state)?)
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let settings = decode(state)?;
        let index = DESCRIPTORS
            .iter()
            .position(|p| p.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        Ok(ParameterValue::Float(settings.0[index]))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = decode(state)?;
        let index = DESCRIPTORS
            .iter()
            .position(|p| p.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        DESCRIPTORS[index].validate(value)?;
        let ParameterValue::Float(value) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };
        settings.0[index] = value;
        encode(settings)
    }
}
impl NativeDeviceDefinition for DelayDefinition {
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        decode(state)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_effect_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        decode(state)?;
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        _context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn hex_lattice::PreparedNativeProcessor>>, DeviceContractError> {
        decode(state)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_runtime_factory(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        if !(8000..=192000).contains(&context.sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = decode(state)?;
        Ok(Some(PreparedProcessorFactory::Owned(Arc::new(Factory {
            settings,
            rate: context.sample_rate,
        }))))
    }
}
struct Factory {
    settings: Settings,
    rate: u32,
}
impl PreparedOwnedProcessorRuntime for Factory {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_pair_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
            + (self.rate as usize + 2) * 2 * std::mem::size_of::<StereoFrame>()
    }
    fn create_runtime(&self) -> Result<Box<dyn OwnedProcessorRuntime>, DeviceContractError> {
        let length = self.rate as usize + 2;
        let ring = || {
            let mut ring = Vec::new();
            ring.try_reserve_exact(length)
                .map_err(|_| DeviceContractError::RuntimeAllocation)?;
            ring.resize(length, [0.0; 2]);
            Ok(ring)
        };
        let current = [
            f64::from(self.settings.0[0]) * f64::from(self.rate) / 1000.0,
            f64::from(self.settings.0[1]),
            f64::from(self.settings.0[2]),
        ];
        let scalar = Scalar {
            next: 0,
            nonzero: 0,
            current,
            remaining: 0,
        };
        let runtime = Runtime {
            settings: self.settings,
            rate: self.rate,
            target: current,
            committed: ring()?,
            scratch: ring()?,
            primary: scalar,
            speculative: scalar,
            dirty_start: 0,
            dirty_len: 0,
            open: false,
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
#[derive(Clone, Copy)]
struct Scalar {
    next: usize,
    nonzero: usize,
    current: [f64; 3],
    remaining: u32,
}
struct Runtime {
    settings: Settings,
    rate: u32,
    target: [f64; 3],
    committed: Vec<StereoFrame>,
    scratch: Vec<StereoFrame>,
    primary: Scalar,
    speculative: Scalar,
    dirty_start: usize,
    dirty_len: usize,
    open: bool,
}
fn finite(value: f64) -> f32 {
    value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
}
impl Runtime {
    fn copy_dirty(
        source: &[StereoFrame],
        destination: &mut [StereoFrame],
        start: usize,
        length: usize,
    ) {
        let first = length.min(source.len() - start);
        destination[start..start + first].copy_from_slice(&source[start..start + first]);
        destination[..length - first].copy_from_slice(&source[..length - first]);
    }
}
impl OwnedProcessorRuntime for Runtime {
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
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        if !self.open {
            self.begin_block();
        }
        let length = self.scratch.len();
        for frame in frames {
            let s = &mut self.speculative;
            if s.remaining > 0 {
                for (current, target) in s.current.iter_mut().zip(self.target) {
                    *current += (target - *current) / f64::from(s.remaining);
                }
                s.remaining -= 1;
                if s.remaining == 0 {
                    s.current = self.target;
                }
            }
            let [time, feedback, mix] = s.current;
            let whole = time.floor() as usize;
            let fraction = time - whole as f64;
            let recent = (s.next + length - whole) % length;
            let older = if recent == 0 { length - 1 } else { recent - 1 };
            let mut write = [0.0; 2];
            for lane in 0..2 {
                let dry = if frame[lane].is_finite() {
                    f64::from(frame[lane])
                } else {
                    0.0
                };
                let wet = f64::from(self.scratch[recent][lane]) * (1.0 - fraction)
                    + f64::from(self.scratch[older][lane]) * fraction;
                let value = finite(dry + wet * feedback);
                write[lane] = if value.abs() < 1e-12 { 0.0 } else { value };
                frame[lane] = finite(dry * (1.0 - mix) + wet * mix);
            }
            s.nonzero -= usize::from(self.scratch[s.next] != [0.0; 2]);
            s.nonzero += usize::from(write != [0.0; 2]);
            self.scratch[s.next] = write;
            s.next += 1;
            if s.next == length {
                s.next = 0;
            }
            self.dirty_len = self.dirty_len.saturating_add(1).min(length);
        }
    }
    fn commit_block(&mut self) {
        if !self.open {
            return;
        }
        Self::copy_dirty(
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
        Self::copy_dirty(
            &self.committed,
            &mut self.scratch,
            self.dirty_start,
            self.dirty_len,
        );
        self.speculative = self.primary;
        self.dirty_len = 0;
        self.open = false;
    }
    fn reset(&mut self) {
        self.committed.fill([0.0; 2]);
        self.scratch.fill([0.0; 2]);
        self.primary = Scalar {
            next: 0,
            nonzero: 0,
            current: self.target,
            remaining: 0,
        };
        self.speculative = self.primary;
        self.dirty_start = 0;
        self.dirty_len = 0;
        self.open = false;
    }
    fn committed_is_quiescent(&self) -> bool {
        self.primary.nonzero == 0
    }
    fn speculative_is_quiescent(&self) -> bool {
        self.speculative.nonzero == 0
    }
    fn can_inherit_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| !self.open && !old.open && self.rate == old.rate)
    }
    fn inherit_from(&mut self, previous: &dyn OwnedProcessorRuntime) {
        if !self.can_inherit_from(previous) {
            return;
        }
        if let Some(old) = previous.as_any().downcast_ref::<Self>() {
            self.committed.copy_from_slice(&old.committed);
            self.scratch.copy_from_slice(&old.committed);
            self.primary = old.primary;
            if self.settings != old.settings {
                self.primary.remaining = self.rate.div_ceil(100);
            }
            self.speculative = self.primary;
            self.dirty_len = 0;
            self.dirty_start = self.primary.next;
        }
    }
    fn can_retarget_parameters_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        self.can_inherit_from(previous)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
#[cfg(test)]
mod tests;
