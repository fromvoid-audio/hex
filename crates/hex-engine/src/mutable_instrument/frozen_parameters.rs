//! Control-reserved scalar targets carried by the existing runtime owner.
//!
//! Attached rendering ignores this staging. Handoff observes targets read-only
//! during planning, then captures current committed values before owner moves.
//! Moving the runtime into a tail activates consumption; existing tails retain
//! their frame. No program, resource lease or DSP history is retained here.
use super::*;
use hex_lattice::NativeParameterAutomation;
use hex_lattice_contract::{
    MAX_PARAMETERS_PER_DEVICE, ParameterDescriptor, ParameterId, ParameterValue,
};

#[derive(Default)]
pub(super) struct FrozenParameters {
    ids: Vec<ParameterId>,
    values: Vec<Option<ParameterValue>>,
    unclamped_target: Option<(ParameterId, f64)>,
    ready: bool,
}
impl FrozenParameters {
    pub(super) fn quote(count: usize) -> Option<usize> {
        (count <= MAX_PARAMETERS_PER_DEVICE).then_some(())?;
        count.checked_mul(
            std::mem::size_of::<ParameterId>()
                .checked_add(std::mem::size_of::<Option<ParameterValue>>())?,
        )
    }
    pub(super) fn new(
        descriptors: &[ParameterDescriptor],
    ) -> Result<Self, MutableInstrumentRuntimeError> {
        let quote = Self::quote(descriptors.len()).ok_or(MutableInstrumentRuntimeError::Budget)?;
        let mut result = Self::default();
        result
            .ids
            .try_reserve_exact(descriptors.len())
            .map_err(|_| MutableInstrumentRuntimeError::Allocation)?;
        result
            .values
            .try_reserve_exact(descriptors.len())
            .map_err(|_| MutableInstrumentRuntimeError::Allocation)?;
        result
            .ids
            .extend(descriptors.iter().map(|descriptor| descriptor.id));
        result.values.resize(descriptors.len(), None);
        if result.storage_bytes().is_none_or(|bytes| bytes > quote) {
            return Err(MutableInstrumentRuntimeError::Budget);
        }
        Ok(result)
    }
    pub(super) fn storage_bytes(&self) -> Option<usize> {
        self.ids
            .capacity()
            .checked_mul(std::mem::size_of::<ParameterId>())?
            .checked_add(
                self.values
                    .capacity()
                    .checked_mul(std::mem::size_of::<Option<ParameterValue>>())?,
            )
    }
    pub(super) fn empty(&self) -> bool {
        self.ids.capacity() == 0 && self.values.capacity() == 0
    }
    pub(super) fn visit_empty<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        if !self.empty() {
            return Err(crate::AudioStorageError::UnsupportedGraph);
        }
        ledger.vec(&self.ids)?;
        ledger.vec(&self.values)
    }
    fn preflight(
        &self,
        pair: &NativeInstrumentRuntimePair,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        if self.ids.is_empty() {
            return Ok(());
        }
        let mut targets = [None; MAX_PARAMETERS_PER_DEVICE];
        let output = targets
            .get_mut(..self.ids.len())
            .ok_or(MutableInstrumentRuntimeError::Budget)?;
        pair.copy_committed_parameter_targets(&self.ids, output)
            .map_err(map_runtime_error)?;
        pair.committed_unclamped_modulation_target()
            .map(|_| ())
            .map_err(map_runtime_error)
    }
    fn capture(
        &mut self,
        pair: &NativeInstrumentRuntimePair,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        if self.ids.is_empty() {
            return Ok(());
        }
        let unclamped_target = pair
            .committed_unclamped_modulation_target()
            .map_err(map_runtime_error)?;
        pair.copy_committed_parameter_targets(&self.ids, &mut self.values)
            .map_err(map_runtime_error)?;
        self.unclamped_target = unclamped_target;
        self.ready = true;
        Ok(())
    }
    pub(super) fn reset(&mut self) {
        self.values.fill(None);
        self.unclamped_target = None;
        self.ready = false;
    }
}
impl NativeParameterAutomation for FrozenParameters {
    fn normalized_before_clamp(&self, parameter: ParameterId, _: usize) -> Option<f64> {
        self.unclamped_target
            .filter(|(id, _)| *id == parameter)
            .map(|(_, value)| value)
    }

    fn parameter_ids(&self) -> &[ParameterId] {
        &self.ids
    }
    fn value_at(&self, id: ParameterId, _: usize) -> Option<ParameterValue> {
        self.ids
            .binary_search(&id)
            .ok()
            .and_then(|index| self.values.get(index))
            .copied()
            .flatten()
    }
}
impl MutableInstrumentTrackRuntime {
    pub(crate) fn preflight_tail_parameters(&self) -> Result<(), MutableInstrumentRuntimeError> {
        self.frozen_parameters.preflight(&self.pair)
    }
    pub(crate) fn capture_tail_parameters(&mut self) -> Result<(), MutableInstrumentRuntimeError> {
        #[cfg(test)]
        if CAPTURE_FAILURE.with(|slot| match slot.get() {
            Some(0) => {
                slot.set(None);
                true
            }
            Some(count) => {
                slot.set(Some(count - 1));
                false
            }
            None => false,
        }) {
            return Err(MutableInstrumentRuntimeError::Budget);
        }
        self.frozen_parameters.capture(&self.pair)
    }
    /// Restore the pure-value frame before propagating either success or failure.
    /// Attached rendering never consumes this staging, including after refusal.
    pub(crate) fn with_frozen_parameters<R>(
        &mut self,
        render: impl FnOnce(&mut Self, Option<&dyn NativeParameterAutomation>) -> R,
    ) -> Result<R, MutableInstrumentRuntimeError> {
        if self.frozen_parameters.ids.is_empty() {
            return Ok(render(self, None));
        }
        if !self.frozen_parameters.ready {
            return Err(map_runtime_error(
                NativeInstrumentRuntimeError::InvalidParameterAutomation,
            ));
        }
        let frame = std::mem::take(&mut self.frozen_parameters);
        // The detached tail consumes the last committed effective values.
        // Keep custody of the inactive source bank through the callback.
        let modulation = self.modulation.take();
        let result = render(self, Some(&frame));
        self.modulation = modulation;
        self.frozen_parameters = frame;
        Ok(result)
    }
}

impl MutableInstrumentTrackRuntime {
    /// Tail subdivision shares the ordinary event/gain transaction but has no
    /// new clip-volume lookup. Existing left-limit occurrence gains stay frozen.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn process_frozen_tail_segment(
        &mut self,
        track: Option<TrackId>,
        performance: u64,
        frames: &mut [[f32; 2]],
        onsets: &mut MutableOnsetBuffer,
        events: &mut Vec<NativeInstrumentEvent>,
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        values: Option<&dyn NativeParameterAutomation>,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        if values.is_none() {
            return self.process_segment_inner(
                track,
                performance,
                performance,
                frames,
                onsets,
                &[],
                events,
                clock,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
                None,
                None,
            );
        }
        self.process_volume_scope(
            track,
            performance,
            performance,
            frames,
            onsets,
            &[],
            events,
            clock,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
            None,
            values,
        )
    }
}

#[cfg(test)]
thread_local! {
    static CAPTURE_FAILURE: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
#[cfg(test)]
pub(crate) fn with_capture_failure<R>(after: usize, run: impl FnOnce() -> R) -> R {
    struct Restore(Option<usize>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CAPTURE_FAILURE.with(|slot| slot.set(self.0));
        }
    }
    let _restore = Restore(CAPTURE_FAILURE.with(|slot| slot.replace(Some(after))));
    run()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_storage_is_bounded_and_render_error_restores_frame() {
        let registry = hex_devices::native_instrument_registry().unwrap();
        let tuning = hex_lattice::TuningTable::default();
        let instrument = registry
            .prepare_instrument(
                &registry
                    .default_state(hex_devices::fm4::device::FM4_TYPE_ID)
                    .unwrap(),
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48000,
                    tuning: &tuning,
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap();
        let factory = instrument.prepare_mutable_runtime().unwrap().unwrap();
        let mut runtime = MutableInstrumentTrackRuntime::new(factory.as_ref(), 48000).unwrap();
        assert_eq!(
            runtime.frozen_parameters.storage_bytes(),
            FrozenParameters::quote(4)
        );
        assert_eq!(
            MutableInstrumentTrackRuntime::checkpoint_storage_upper_bound(factory.as_ref(), 48000),
            runtime
                .declared_pair_bytes()
                .checked_add(runtime.ledger_capacity_bytes().unwrap())
                .and_then(|bytes| bytes.checked_add(FrozenParameters::quote(4).unwrap()))
        );
        assert!(runtime.with_frozen_parameters(|_, _| ()).is_err());
        runtime.capture_tail_parameters().unwrap();
        let before = runtime.frozen_parameters.values.clone();
        let failed: Result<(), ()> = runtime
            .with_frozen_parameters(|_, values| {
                assert_eq!(values.unwrap().parameter_ids().len(), 4);
                Err(())
            })
            .unwrap();
        assert!(failed.is_err());
        assert_eq!(runtime.frozen_parameters.values, before);
        assert!(runtime.frozen_parameters.ready);
        runtime
            .with_frozen_parameters(|_, values| {
                assert_eq!(
                    values.unwrap().value_at(ParameterId::from_static(29), 0),
                    before[0]
                )
            })
            .unwrap();
        let bytes = runtime.frozen_parameters.storage_bytes();
        runtime.reset();
        assert_eq!(runtime.frozen_parameters.storage_bytes(), bytes);
        assert!(!runtime.frozen_parameters.ready);
        assert!(FrozenParameters::new(&[]).unwrap().empty());
        assert!(FrozenParameters::quote(MAX_PARAMETERS_PER_DEVICE + 1).is_none());
    }
}
