//! Shared single-owner `OwnedProcessorRuntime` adapter for fixed-inline
//! `Copy` native effects.
//!
//! Replaces the legacy two-box twin-copy path (`RuntimeOwnership::Legacy`)
//! with a single boxed `InlineOwnedRuntime<R>` that keeps committed and
//! speculative copies inline, avoids redundant copies on clean `begin_block`,
//! and forwards every capability hook without per-device boilerplate.

use hex_lattice::{
    DeviceResources, DynamicsMeter, NativeInstrumentClock, NativeParameterAutomation,
    NativeProcessorRuntime, OutputTail, OwnedProcessorRuntime, PreparedNativeProcessor,
    PreparedOwnedProcessorRuntime, PreparedProcessorFactory, PreparedRuntimeDevice,
    StereoAuxiliaryBlock, StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, ParameterDescriptor, ParameterId, ParameterSemantics,
};
use std::{any::Any, sync::Arc};

/// Control-prepared inline effect factory whose runtime is `Copy`.
pub(crate) trait InlinePreparedProcessor:
    PreparedNativeProcessor + Send + Sync + 'static
{
    type Runtime: NativeProcessorRuntime + Copy + 'static;

    fn create_inline_runtime(&self) -> Self::Runtime;
}

macro_rules! impl_inline_owned_factory {
    ($($prepared:ty),+ $(,)?) => {
        $(
            impl PreparedOwnedProcessorRuntime for $prepared {
                fn reserved_bypass_latency_frames(&self) -> Option<u32> {
                    PreparedNativeProcessor::supports_latency_aligned_bypass(self)
                        .then(|| {
                            PreparedNativeProcessor::algorithmic_latency_frames(self)
                                .filter(|&n| n > 0)
                        })
                        .flatten()
                }

                fn supports_smoothed_bypass(&self) -> bool {
                    PreparedNativeProcessor::supports_smoothed_bypass(self)
                }

                fn supports_power_off(&self) -> bool {
                    PreparedNativeProcessor::supports_power_off(self)
                }

                fn parameter_automation_descriptor(
                    &self,
                    parameter: ParameterId,
                ) -> Option<ParameterDescriptor> {
                    PreparedNativeProcessor::parameter_automation_descriptor(self, parameter)
                }

                fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
                    PreparedNativeProcessor::supports_parameter_automation(self, parameter)
                }

                fn parameter_modulation_semantics(
                    &self,
                    parameter: ParameterId,
                ) -> Option<ParameterSemantics> {
                    PreparedNativeProcessor::parameter_modulation_semantics(self, parameter)
                }

                fn runtime_version(&self) -> u32 {
                    PreparedNativeProcessor::runtime_version(self)
                }

                fn declared_pair_bytes(&self) -> usize {
                    std::mem::size_of::<InlineOwnedRuntime<<Self as InlinePreparedProcessor>::Runtime>>()
                }

                fn create_runtime(
                    &self,
                ) -> Result<Box<dyn OwnedProcessorRuntime>, DeviceContractError> {
                    Ok(Box::new(InlineOwnedRuntime::new(
                        InlinePreparedProcessor::create_inline_runtime(self),
                    )))
                }

                fn output_tail(&self) -> OutputTail {
                    PreparedNativeProcessor::output_tail(self)
                }

                fn algorithmic_latency_frames(&self) -> Option<u32> {
                    PreparedNativeProcessor::algorithmic_latency_frames(self)
                }

                fn frequency_response_at_hz(&self, frequency_hz: f32) -> Option<[f32; 2]> {
                    PreparedNativeProcessor::frequency_response_at_hz(self, frequency_hz)
                }
            }
        )+
    };
}

impl_inline_owned_factory!(
    crate::auto_pan::PreparedAutoPan,
    crate::channel_eq::PreparedChannelEq,
    crate::compressor::PreparedCompressor,
    crate::eq_three::PreparedEqThree,
    crate::filter::PreparedFilter,
    crate::gate::PreparedGate,
    crate::overdrive::PreparedOverdrive,
    crate::rattle::PreparedRattle,
    crate::redux::PreparedRedux,
    crate::saturator::PreparedSaturator,
    crate::utility::PreparedUtilityFilterRuntime,
);

/// Single-owner transactional runtime for a `Copy` inline processor `R`.
pub struct InlineOwnedRuntime<R: NativeProcessorRuntime + Copy + 'static> {
    committed: R,
    speculative: R,
    dirty: bool,
}

impl<R: NativeProcessorRuntime + Copy + 'static> InlineOwnedRuntime<R> {
    pub(crate) fn new(initial: R) -> Self {
        Self {
            committed: initial,
            speculative: initial,
            dirty: false,
        }
    }

    /// Exact inline allocation size of this single-owner runtime box.
    pub fn control_storage(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

impl<R: NativeProcessorRuntime + Copy + 'static> OwnedProcessorRuntime for InlineOwnedRuntime<R> {
    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        Some(std::mem::size_of::<Self>())
    }

    fn begin_block(&mut self) {
        if self.dirty {
            self.speculative = self.committed;
            self.dirty = false;
        }
    }

    fn process_block(&mut self, frames: &mut [StereoFrame], resources: &dyn DeviceResources) {
        self.dirty = true;
        self.speculative.process_block(frames, resources);
    }

    fn process_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        resources: &dyn DeviceResources,
    ) {
        self.dirty = true;
        self.speculative
            .process_block_with_clock(frames, clock, resources);
    }

    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        resources: &dyn DeviceResources,
    ) {
        self.dirty = true;
        self.speculative
            .process_bypassed_block_with_clock(frames, clock, resources);
    }

    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        self.committed.supports_parameter_automation(parameter)
    }

    fn process_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        automation: &dyn NativeParameterAutomation,
        resources: &dyn DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.dirty = true;
        self.speculative
            .process_block_with_automation(frames, clock, automation, resources)
    }

    fn process_bypassed_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        automation: &dyn NativeParameterAutomation,
        resources: &dyn DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.dirty = true;
        self.speculative
            .process_bypassed_block_with_automation(frames, clock, automation, resources)
    }

    fn process_block_with_auxiliary(
        &mut self,
        block: StereoAuxiliaryBlock<'_>,
        clock: Option<&dyn NativeInstrumentClock>,
        automation: Option<&dyn NativeParameterAutomation>,
        bypassed: bool,
        resources: &dyn DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.dirty = true;
        self.speculative
            .process_block_with_auxiliary(block, clock, automation, bypassed, resources)
    }

    fn commit_block(&mut self) {
        if self.dirty {
            self.committed = self.speculative;
            self.dirty = false;
        }
    }

    fn rollback_block(&mut self) {
        if self.dirty {
            self.speculative = self.committed;
            self.dirty = false;
        }
    }

    fn reset(&mut self) {
        self.committed.reset();
        self.speculative = self.committed;
        self.dirty = false;
    }

    fn begin_output_tail(&mut self) {
        self.committed.begin_output_tail();
        self.speculative.begin_output_tail();
    }

    fn committed_is_quiescent(&self) -> bool {
        self.committed.is_quiescent()
    }

    fn speculative_is_quiescent(&self) -> bool {
        self.speculative.is_quiescent()
    }

    fn can_inherit_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| self.committed.can_copy_history_from(&old.committed))
    }

    fn inherit_from(&mut self, previous: &dyn OwnedProcessorRuntime) {
        if let Some(old) = previous.as_any().downcast_ref::<Self>() {
            self.committed.copy_history_from(&old.committed);
            self.speculative = self.committed;
            self.dirty = false;
        }
    }

    fn can_retarget_parameters_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| self.committed.can_retarget_parameters_from(&old.committed))
    }

    fn supports_manual_host_matrix_retarget(&self) -> bool {
        self.committed.supports_manual_host_matrix_retarget()
    }

    fn can_retarget_source_host_matrix_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|old| {
            self.committed
                .can_retarget_source_host_matrix_from(&old.committed)
        })
    }

    fn can_retarget_source_parameters_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|old| {
            self.committed
                .can_retarget_source_parameters_from(&old.committed)
        })
    }

    fn permits_memoryless_modulation_handoff(&self) -> bool {
        self.committed.permits_memoryless_modulation_handoff()
    }

    fn supports_detached_source_rejoin(&self) -> bool {
        self.committed.supports_detached_source_rejoin()
    }

    fn transition_busy_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| self.committed.transition_busy_from(&old.committed))
    }

    fn settle_idle_transitions(&mut self) {
        self.committed.settle_idle_transitions();
        self.speculative = self.committed;
        self.dirty = false;
    }

    fn take_dynamics_meter(&mut self) -> Option<DynamicsMeter> {
        let meter = self.committed.take_dynamics_meter();
        if meter.is_some() {
            // Committed telemetry was drained; resync speculative before the next block.
            self.dirty = true;
        }
        meter
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub(crate) fn owned_factory<P: PreparedOwnedProcessorRuntime + 'static>(
    factory: P,
) -> PreparedProcessorFactory {
    PreparedProcessorFactory::Owned(Arc::new(factory))
}

pub(crate) fn prepared_owned_runtime<P: PreparedOwnedProcessorRuntime + 'static>(
    factory: P,
) -> PreparedRuntimeDevice {
    PreparedRuntimeDevice {
        matrix: StereoMatrix::identity(),
        processor: Some(owned_factory(factory)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        auto_pan::AutoPanDefinition, compressor::CompressorDefinition, filter::FilterDefinition,
        gate::GateDefinition, overdrive::OverdriveDefinition, rattle::RattleDefinition,
        saturator::SaturatorDefinition, utility::UtilityDefinition,
    };
    use hex_lattice::{
        DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeDeviceDefinition,
        ProcessorChainOwner, ProcessorInstanceIdentity,
    };
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct CounterRuntime {
        gain: f32,
        calls: u32,
        pending_meter: Option<DynamicsMeter>,
    }

    impl NativeProcessorRuntime for CounterRuntime {
        fn process_block(&mut self, frames: &mut [StereoFrame], _resources: &dyn DeviceResources) {
            self.calls += 1;
            for frame in frames {
                frame[0] *= self.gain;
                frame[1] *= self.gain;
            }
            self.pending_meter = Some(DynamicsMeter {
                reduction_db: [-3.0 * self.calls as f32; 2],
                clipped: false,
                post_input_peak: 0.5,
            });
        }

        fn reset(&mut self) {
            self.calls = 0;
            self.pending_meter = None;
        }

        fn is_quiescent(&self) -> bool {
            self.calls == 0
        }

        fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
            previous.as_any().is::<Self>()
        }

        fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
            if let Some(old) = previous.as_any().downcast_ref::<Self>() {
                self.calls = old.calls;
                self.pending_meter = old.pending_meter;
            }
        }

        fn take_dynamics_meter(&mut self) -> Option<DynamicsMeter> {
            self.pending_meter.take()
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[test]
    fn inline_owned_runtime_commits_rolls_back_inherits_and_resyncs_drained_meter() {
        let mut rt = InlineOwnedRuntime::new(CounterRuntime {
            gain: 0.5,
            calls: 0,
            pending_meter: None,
        });
        assert_eq!(
            rt.fixed_storage_pair_bytes(),
            Some(std::mem::size_of::<InlineOwnedRuntime<CounterRuntime>>())
        );
        assert!(rt.committed_is_quiescent());

        // Rollback preserves committed state.
        rt.begin_block();
        let mut frames = [[1.0, -1.0]; 2];
        rt.process_block(&mut frames, &EMPTY_DEVICE_RESOURCES);
        assert_eq!(frames, [[0.5, -0.5]; 2]);
        assert!(!rt.speculative_is_quiescent());
        assert!(rt.committed_is_quiescent());
        assert!(rt.take_dynamics_meter().is_none());
        rt.rollback_block();
        assert!(rt.speculative_is_quiescent());

        // Commit advances committed state and exposes meter.
        rt.begin_block();
        rt.process_block(&mut frames, &EMPTY_DEVICE_RESOURCES);
        rt.commit_block();
        assert!(!rt.committed_is_quiescent());
        let drained = rt.take_dynamics_meter().expect("committed meter");
        assert_eq!(drained.reduction_db, [-3.0; 2]);
        assert!(rt.dirty, "draining meter must schedule speculative resync");
        rt.begin_block();
        assert!(!rt.dirty);
        assert!(rt.speculative.pending_meter.is_none());

        // Inherit transfers committed history into a fresh successor.
        let mut next = InlineOwnedRuntime::new(CounterRuntime {
            gain: 0.25,
            calls: 0,
            pending_meter: None,
        });
        assert!(next.can_inherit_from(&rt));
        next.inherit_from(&rt);
        assert_eq!(next.committed.calls, 1);
        assert_eq!(next.speculative.calls, 1);
    }

    #[test]
    fn all_eight_inline_effects_prepare_owned_factories() {
        let tuning = hex_lattice::TuningTable::default();
        let context = DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &tuning,
            resources: &EMPTY_DEVICE_RESOURCES,
        };
        let identity =
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(1), 1).expect("identity");
        let utility_state = {
            let state = UtilityDefinition.default_state().expect("default utility");
            UtilityDefinition
                .patch_parameter(
                    &state,
                    crate::utility::parameter::DC_REMOVE,
                    ParameterValue::Bool(true),
                )
                .expect("enable dc remove")
        };
        let defs: [(
            &str,
            &dyn NativeDeviceDefinition,
            hex_lattice_contract::NativeDeviceState,
        ); 8] = [
            (
                "filter",
                &FilterDefinition,
                FilterDefinition.default_state().expect("default filter"),
            ),
            (
                "gate",
                &GateDefinition,
                GateDefinition.default_state().expect("default gate"),
            ),
            (
                "compressor",
                &CompressorDefinition,
                CompressorDefinition
                    .default_state()
                    .expect("default compressor"),
            ),
            (
                "auto_pan",
                &AutoPanDefinition,
                AutoPanDefinition.default_state().expect("default auto_pan"),
            ),
            (
                "rattle",
                &RattleDefinition,
                RattleDefinition.default_state().expect("default rattle"),
            ),
            (
                "saturator",
                &SaturatorDefinition,
                SaturatorDefinition
                    .default_state()
                    .expect("default saturator"),
            ),
            (
                "overdrive",
                &OverdriveDefinition,
                OverdriveDefinition
                    .default_state()
                    .expect("default overdrive"),
            ),
            ("utility", &UtilityDefinition, utility_state),
        ];

        for (name, def, state) in defs {
            let factory = def
                .prepare_runtime_factory(&state, &context)
                .expect("prepare_runtime_factory")
                .expect("stateful factory");
            assert!(
                factory.is_owned(),
                "{name} prepare_runtime_factory must return Owned"
            );
            let auto_param = def
                .descriptors()
                .iter()
                .find(|p| {
                    def.supports_parameter_automation(&state, p.id)
                        .unwrap_or(false)
                })
                .map(|p| p.id)
                .expect("automated parameter");
            let automated = def
                .prepare_automated_runtime_instance(identity, &state, &context, &[auto_param])
                .expect("prepare_automated_runtime_instance");
            assert!(
                automated
                    .processor
                    .as_ref()
                    .is_some_and(PreparedProcessorFactory::is_owned),
                "{name} prepare_automated_runtime_instance must return Owned"
            );
        }
    }
}
