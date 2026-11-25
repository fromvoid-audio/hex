//! # Purpose
//! Own validated native processor/instrument registries, borrowed resource contracts and bounded
//! runtime-history transactions and explicit borrowed foreign processing.
//!
//! # Dependencies
//! Uses hex-lattice-contract, hex-dsp and hex-tuning; providers supply original DSP through
//! explicit preparation capabilities.
//!
//! # Does not own
//! Does not own canonical edits, device I/O, host source leases or foreign-code containment. A
//! resource view or declaration never grants host publication or lifetime authority.
mod aux_input;
mod bypass_ramp;
#[cfg(any(test, feature = "conformance"))]
pub mod conformance;
pub use aux_input::{AuxiliaryBlockError, StereoAuxiliaryBlock};
mod foreign_modulation;
mod foreign_processor;
mod host_input_gain_modulation;
mod host_output_gain_ramp;
mod matrix_ramp;
mod meter;
mod modulation;
mod modulation_adsr;
mod modulation_ahd_release;
#[cfg(test)]
mod modulation_graph_tests;
mod modulation_note_counter;
mod modulation_note_events;
mod modulation_note_gate;
mod modulation_ramp;
mod modulation_runtime;
mod modulation_source_routes;
mod modulation_sources;
mod modulation_steps;
mod modulator_registry;
mod native_move;
mod native_reorder;
mod owned_processor;
mod parameter_automation;
mod prepared_modulation;
pub use prepared_modulation::{MAX_PREPARED_MODULATION_BYTES, PreparedModulationState};
#[cfg(test)]
mod powered_off_tests;
#[cfg(test)]
mod static_processing_tests;
mod tail_split;
pub use bypass_ramp::{MAX_MAIN_PASS_DELAY_FRAMES, MainPassDelay};
pub use foreign_processor::{
    ForeignAutomation, ForeignAutomationProgram, ForeignAutomationSource, ForeignNote,
    ForeignNoteKind, ForeignParameterCapability, ForeignProcessorNode, ForeignProcessors,
    MAX_RETAINED_FOREIGN_PROGRAMS, PreparedForeignAutomation, PreparedForeignProcessor,
    RefuseForeign,
};
pub use parameter_automation::{
    ExternalParameterAutomation, HostAutomationLane, NativeParameterAutomation,
    NativeTransientOperation, NativeTransientValue, NormalizedAutomationValue,
    ParameterAutomationBlock, ParameterAutomationLane, ParameterAutomationPoint,
    ParameterAutomationTouchSpan, PreparedParameterAutomation, PreparedParameterAutomationLane,
    PreparedParameterAutomationPoint,
};
pub use tail_split::{TailSplit, TailSplitObserver};
mod resources;
mod runtime_ownership;
pub use foreign_modulation::{
    ForeignModulationRuntime, ForeignModulationSamples, ForeignModulationTarget,
    MAX_FOREIGN_MODULATION_TARGETS, PreparedForeignModulation,
};
pub use hex_dsp::LfoWaveform;
pub use hex_lattice_contract::ModulationSourceId;
use hex_lattice_contract::{DeviceContractError, DeviceTypeId, MAX_NATIVE_DEVICE_TYPES};
#[cfg(test)]
use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState};
pub use hex_tuning::{TuningTable, Waveform as TuningWaveform};
pub use meter::{DeviceMeter, DevicePeaks, DynamicsMeter, RenderWorkMeter, RenderWorkSample};
pub use modulation::{
    ForeignModulationRoute, ForeignTargetModulationRoute, HostGainModulationRoute,
    MAX_MODULATION_BLOCK_FRAMES, MAX_MODULATION_ROUTES, MAX_MODULATION_SOURCES, ModulationBlock,
    ModulationRoute, ModulationTarget, PreparedModulation, PreparedPressureRoute,
};
pub use modulation_ramp::ModulationRampCurve;
pub use modulation_runtime::ModulationRuntime;
pub use modulation_source_routes::{SourceModulationParameter, SourceModulationRoute};
pub use modulation_sources::{
    ModulationMathOperation, ModulationSourceBank, PreparedModulationSource,
};
pub use modulation_steps::{MAX_MODULATION_STEPS, ModulationStepDirection};
pub use modulator_registry::{
    ModulatorPreparationContext, ModulatorPreparedOutput, NativeModulatorDefinition,
    NativeModulatorRegistry,
};
pub use owned_processor::{
    MAX_RESERVED_BYPASS_FRAMES, OwnedProcessorRuntime, PreparedOwnedProcessorRuntime,
    PreparedProcessorFactory, PreparedRuntimeDevice, PreparedRuntimeInstance,
};
pub use resources::{
    BorrowedDeviceResources, DevicePreparationContext, DeviceResourceIdentity, DeviceResourcePcm,
    DeviceResourceView, DeviceResources, EMPTY_DEVICE_RESOURCES,
};
use runtime_ownership::RuntimeOwnership;
#[cfg(test)]
use std::any::Any as StdAny;

mod analytic_continuation;
pub use analytic_continuation::{
    ANALYTIC_CONTINUATION_WORDS, AnalyticContinuation, AnalyticContinuationError,
    AnalyticContinuationFormat, AnalyticContinuationId, AnalyticParameterFrame,
    MAX_ANALYTIC_CONTINUATION_FRAMES, PreparedAnalyticContinuation,
};

mod instrument;
mod instrument_gain;
mod instrument_pair;
mod instrument_runtime;
pub use instrument::{
    ANALYTIC_NOTE_PAYLOAD_WORDS, AnalyticNoteContinuation, MAX_SHARED_ANALYTIC_CONTEXT_BYTES,
    NativeInstrumentDefinition, NativeInstrumentRegistry, PreparedAnalyticNote,
    PreparedNativeInstrument, PreparedSharedAnalyticContext, PreparedSharedAnalyticNote,
    PreparedStorageDeclaration, SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS,
};
pub use instrument_gain::{
    INSTRUMENT_GAIN_UNITY, MAX_INSTRUMENT_OCCURRENCE_GAIN, NativeInstrumentOccurrenceGain,
};
pub use instrument_pair::{
    NativeOwnedInstrumentRuntimePair, OwnedInstrumentRuntime, PreparedOwnedInstrumentRuntime,
};
pub use instrument_runtime::{
    AnalyticInstrumentHistory, InstrumentHistoryRetargetPreparation, InstrumentParameterRuntime,
    MAX_INSTRUMENT_AUTOMATION_FRAMES, MutableInstrumentRuntime, NativeInstrumentAnalyticSeed,
    NativeInstrumentClock, NativeInstrumentEvent, NativeInstrumentExpression,
    NativeInstrumentRuntimeError, NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
    PreparedInstrumentHistoryRetarget, PreparedMutableInstrumentRuntime,
};

mod adapters;
mod registry;
mod runtime_chain;
mod traits;

pub use adapters::{
    ForeignConfigurationContinuation, NativeExternalAutomation, NativeExternalAutomationSet,
    NativeTransientApplication, NativeTransientInput, NativeTransientProcessing,
};
pub(crate) use adapters::{OffsetAutomation, OffsetClock};
pub use registry::NativeDeviceRegistry;
pub(crate) use runtime_chain::{EntryProcessContext, RuntimeEntry};
pub use runtime_chain::{
    NativeInsertTransition, NativeRemovalTransition, NativeRuntimeChain, NativeRuntimeEntryStorage,
    NativeRuntimeStorage, StageProcessContext,
};
pub use traits::{
    MAX_OWNED_PROCESSOR_PAIR_BYTES, MAX_PROCESSOR_BANK_RUNTIME_BYTES, MAX_PROCESSOR_RUNTIME_BYTES,
    MAX_STATEFUL_PROCESSORS, NativeDeviceDefinition, NativeProcessorAudition,
    NativeProcessorRuntime, NodeSignalPhase, OutputTail, PreparedNativeDevice,
    PreparedNativeProcessor, PreparedProcessorInstance, ProcessorAuditionSelection,
    ProcessorChainOwner, ProcessorInstanceIdentity,
};

pub use hex_dsp::{InvalidMatrix, StereoFrame, StereoMatrix};

#[cfg(test)]
mod compensation_tests;

#[cfg(test)]
mod tests;
