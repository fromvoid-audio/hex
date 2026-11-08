//! Allocation boundary of the public twin transaction, not native device timing.
#![cfg(feature = "allocation-probe")]
use hex_lattice::{
    BorrowedDeviceResources, DeviceResources, InstrumentParameterRuntime, MutableInstrumentRuntime,
    NativeInstrumentClock, NativeInstrumentEvent, NativeInstrumentOccurrenceGain,
    NativeInstrumentRuntimeError as Error, NativeInstrumentRuntimeLimits,
    NativeInstrumentRuntimePair, NativeParameterAutomation, PreparedMutableInstrumentRuntime,
    StereoFrame,
};
use hex_lattice_contract::{
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;
const ID: ParameterId = ParameterId::from_static(1);
const GRANTS: &[ParameterDescriptor] = &[ParameterDescriptor {
    id: ID,
    label: "level",
    kind: ParameterKind::Float {
        min: 0.0,
        max: 2.0,
        unit: None,
    },
    default: ParameterValue::Float(1.0),
    presentation: ParameterPresentation::Linear,
}];
struct Factory;
struct Runtime(f32);
struct Values;
impl NativeParameterAutomation for Values {
    fn parameter_ids(&self) -> &[ParameterId] {
        &[ID]
    }
    fn value_at(&self, _: ParameterId, offset: usize) -> Option<ParameterValue> {
        if offset % 2 == 0 {
            Some(ParameterValue::Float(0.5))
        } else {
            None
        }
    }
}
impl PreparedMutableInstrumentRuntime for Factory {
    fn parameter_automation_descriptors(&self) -> &[ParameterDescriptor] {
        GRANTS
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
    }
    fn max_events_per_block(&self) -> usize {
        1
    }
    fn max_active_occurrences(&self) -> usize {
        1
    }
    fn max_release_tail_frames(&self) -> u64 {
        0
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(Runtime(1.0))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl MutableInstrumentRuntime for Runtime {
    fn parameter_automation(&mut self) -> Option<&mut dyn InstrumentParameterRuntime> {
        Some(self)
    }
    fn preflight_events(
        &self,
        _: &[NativeInstrumentEvent],
        _: usize,
        _: &dyn DeviceResources,
    ) -> Result<(), Error> {
        Ok(())
    }
    fn process_block(
        &mut self,
        _: &[NativeInstrumentEvent],
        _: &mut [StereoFrame],
        _: &dyn DeviceResources,
    ) {
    }
    fn reset(&mut self) {
        self.0 = 1.0;
    }
    fn is_quiescent(&self) -> bool {
        true
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.0 = previous.0;
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl InstrumentParameterRuntime for Runtime {
    fn descriptors(&self) -> &[ParameterDescriptor] {
        GRANTS
    }
    fn preflight(
        &self,
        _: &[NativeInstrumentEvent],
        _: usize,
        _: Option<&dyn NativeInstrumentClock>,
        _: &[NativeInstrumentOccurrenceGain],
        _: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) -> Result<(), Error> {
        Ok(())
    }
    fn process(
        &mut self,
        _: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        _: &[NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) {
        for (offset, frame) in frames.iter_mut().enumerate() {
            self.0 = match values.value_at(ID, offset) {
                Some(ParameterValue::Float(value)) => value,
                _ => 1.0,
            };
            *frame = [self.0; 2];
        }
    }
}
#[test]
fn public_twin_automation_preflight_process_commit_rollback_reset_allocate_and_free_nothing() {
    let limits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: 1024,
        max_events_per_block: 1,
        max_active_occurrences: 1,
        max_tail_frames: 0,
    };
    let mut pair = NativeInstrumentRuntimePair::new(&Factory, limits).unwrap();
    let mut frames = [[0.0; 2]; 4096];
    let resources = BorrowedDeviceResources(&[]);
    let mut results = [Ok(()); 2];
    let measured = allocation_counter::measure(|| {
        pair.begin_block();
        results[0] = pair.process_block_with_clock_and_automation(
            &[],
            &mut frames,
            None,
            &[],
            &Values,
            &resources,
        );
        pair.rollback_block();
        pair.begin_block();
        results[1] = pair.process_block_with_clock_and_automation(
            &[],
            &mut frames,
            None,
            &[],
            &Values,
            &resources,
        );
        pair.commit_block();
        pair.reset();
    });
    assert_eq!(results, [Ok(()); 2]);
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(frames[0], [0.5; 2]);
    assert_eq!(frames[4095], [1.0; 2]);
}
