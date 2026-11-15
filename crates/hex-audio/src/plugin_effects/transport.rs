//! The exact copied endpoint retained by an original native effect. Only an
//! explicitly armed effect owns the additional dense queue and staging memory.
use super::*;
use crate::plugin_worker::NativeWorkerStorage;
use hex_plugin_host::{
    native::{PreparedStorage, Quiescent},
    transport::{
        AcceptedEditorQuantum, ConstructorStorage, DenseEffectCallback, DenseParameterLane,
        ParameterPoint, SegmentReply,
    },
};

pub(super) enum EffectTransport {
    Ordinary(Callback),
    Dense(DenseEffectCallback),
}

impl EffectTransport {
    pub(super) fn reserve_observed_parameter_input(
        &self,
        source: &hex_plugin_host::transport::ParameterInputSource,
    ) -> Option<hex_plugin_host::transport::ParameterInputFence<'_>> {
        match self {
            Self::Ordinary(callback) => {
                callback.reserve_observed_parameter_input_on_callback(source)
            }
            Self::Dense(callback) => callback.reserve_observed_parameter_input_on_callback(source),
        }
    }

    pub(super) fn matches_worker(&self, worker: &NativeWorker) -> bool {
        match self {
            Self::Ordinary(callback) => worker.owns_callback(callback),
            Self::Dense(callback) => worker.owns_dense_effect_callback(callback),
        }
    }

    pub(super) fn storage_on_control(&self, worker: &NativeWorker) -> Option<ConstructorStorage> {
        match self {
            Self::Ordinary(callback) => worker.transport_storage_on_control(callback),
            Self::Dense(callback) => worker.dense_effect_transport_storage_on_control(callback),
        }
    }

    pub(super) fn source_storage_on_control(
        &self,
        worker: &NativeWorker,
    ) -> Option<PreparedStorage> {
        match self {
            Self::Ordinary(callback) => worker.retained_source_storage_on_control(callback),
            Self::Dense(callback) => {
                worker.retained_dense_effect_source_storage_on_control(callback)
            }
        }
    }

    pub(super) fn worker_storage_on_control(
        &self,
        worker: &NativeWorker,
    ) -> Option<NativeWorkerStorage> {
        match self {
            Self::Ordinary(callback) => worker.constructor_storage_on_control(callback),
            Self::Dense(callback) => worker.constructor_dense_effect_storage_on_control(callback),
        }
    }

    #[allow(clippy::result_large_err)]
    pub(super) fn into_quiescent_on_control(
        self,
        worker: NativeWorker,
    ) -> Result<Quiescent, NativeWorkerStartFailure> {
        match self {
            Self::Ordinary(callback) => worker.into_quiescent_on_control(callback),
            Self::Dense(callback) => worker.into_quiescent_dense_effect_on_control(callback),
        }
    }

    pub(super) fn begin_physical_callback(&mut self, frames: usize) -> Result<(), Fault> {
        match self {
            Self::Ordinary(callback) => callback.begin_physical_callback(frames),
            Self::Dense(callback) => callback.begin_physical_callback(frames),
        }
    }

    #[allow(clippy::too_many_arguments)] // Borrowed request data stays within the physical extent.
    pub(super) fn stage_effect_segment(
        &mut self,
        offset: usize,
        input: &[[f32; 2]],
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        frames: &mut [[f32; 2]],
        timing: SegmentTiming<impl FnMut(usize) -> Option<ProcessTiming>>,
        origins: EffectSegmentOrigins<'_>,
        lane: Option<DenseParameterLane<'_>>,
    ) -> Result<SegmentReply, Fault> {
        match self {
            Self::Ordinary(callback) if lane.is_none() => callback
                .stage_effect_segment(offset, input, events, parameters, frames, timing, origins),
            Self::Ordinary(callback) => {
                callback.reject(Fault::InvalidParameters);
                frames.fill([0.0; 2]);
                Err(Fault::InvalidParameters)
            }
            Self::Dense(callback) => callback.stage_effect_segment(
                offset, input, events, parameters, frames, timing, origins, lane,
            ),
        }
    }

    pub(super) fn finish(&mut self, accepted: bool) -> Result<(), Fault> {
        match self {
            Self::Ordinary(callback) => callback.finish(accepted),
            Self::Dense(callback) => callback.finish(accepted),
        }
    }

    pub(super) fn end_staging_at_pause(&mut self, frame: usize) {
        match self {
            Self::Ordinary(callback) => callback.end_staging_at_pause(frame),
            Self::Dense(callback) => callback.end_staging_at_pause(frame),
        }
    }

    pub(super) fn take_accepted_editor_quantum(&mut self) -> Option<AcceptedEditorQuantum> {
        match self {
            Self::Ordinary(callback) => callback.take_accepted_editor_quantum(),
            Self::Dense(callback) => callback.take_accepted_editor_quantum(),
        }
    }

    pub(super) fn take_accepted_manual_parameter(&mut self) -> Option<AcceptedManualParameter> {
        match self {
            Self::Ordinary(callback) => callback.take_accepted_manual_parameter(),
            Self::Dense(callback) => callback.take_accepted_manual_parameter(),
        }
    }

    pub(super) fn reject(&mut self, fault: Fault) {
        match self {
            Self::Ordinary(callback) => callback.reject(fault),
            Self::Dense(callback) => callback.reject(fault),
        }
    }

    #[cfg(any(test, feature = "allocation-probe"))]
    pub(super) fn fault(&self) -> Option<Fault> {
        match self {
            Self::Ordinary(callback) => callback.fault(),
            Self::Dense(callback) => callback.fault(),
        }
    }

    pub(super) fn is_revoked(&self) -> bool {
        match self {
            Self::Ordinary(callback) => callback.is_revoked(),
            Self::Dense(callback) => callback.is_revoked(),
        }
    }

    pub(super) fn has_complete_staged_callback(&self) -> bool {
        match self {
            Self::Ordinary(callback) => callback.has_complete_staged_callback(),
            Self::Dense(callback) => callback.has_complete_staged_callback(),
        }
    }

    pub(super) fn dispatched_blocks_on_control(&self) -> u64 {
        match self {
            Self::Ordinary(callback) => callback.dispatched_blocks_on_control(),
            Self::Dense(callback) => callback.dispatched_blocks_on_control(),
        }
    }

    #[cfg(feature = "allocation-probe")]
    pub(super) fn committed_position_for_test(&self) -> u64 {
        match self {
            Self::Ordinary(callback) => callback.committed_position_for_test(),
            Self::Dense(callback) => callback.committed_position_for_test(),
        }
    }

    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn pending_requests_for_test(&self) -> usize {
        match self {
            Self::Ordinary(callback) => callback.pending_requests_for_test(),
            Self::Dense(callback) => callback.pending_requests_for_test(),
        }
    }
}
