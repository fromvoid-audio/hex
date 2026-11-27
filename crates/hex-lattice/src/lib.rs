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
mod audio_rack;
mod aux_input;
mod bypass_ramp;
#[cfg(any(test, feature = "conformance"))]
pub mod conformance;
pub use audio_rack::{
    AUDIO_RACK_BRANCH_COUNT, AUDIO_RACK_MAX_BLOCK_FRAMES, AUDIO_RACK_MAX_BRANCH_GAIN,
    AUDIO_RACK_MAX_COMPENSATION_DELAY_FRAMES, AUDIO_RACK_MAX_DEPTH,
    AUDIO_RACK_MAX_DEVICES_PER_BRANCH, AUDIO_RACK_MAX_MACROS, AUDIO_RACK_MAX_MAPPINGS_PER_MACRO,
    AUDIO_RACK_MAX_NESTED_RACKS_PER_BRANCH, AUDIO_RACK_MAX_PAIR_BYTES,
    AUDIO_RACK_MAX_TOTAL_DEVICES, AUDIO_RACK_MAX_VARIATION_NAME_BYTES, AUDIO_RACK_MAX_VARIATIONS,
    AUDIO_RACK_MAX_WORK_UNITS_PER_BLOCK, AUDIO_RACK_STATE_VERSION,
    AUDIO_RACK_STATE_VERSION_V1_FLAT, AUDIO_RACK_STATE_VERSION_V2_NESTED,
    AUDIO_RACK_STATE_VERSION_V3_MACROS, AUDIO_RACK_TYPE_ID, AudioRackBranchSpec,
    AudioRackDeviceSpec, AudioRackEdit, AudioRackError, AudioRackHistory, AudioRackLimits,
    AudioRackMacroMapping, AudioRackMacroPolarity, AudioRackMacroPrecedence, AudioRackMacroSpec,
    AudioRackMacroTarget, AudioRackMacroVariation, AudioRackMacroVariationEntry, AudioRackRuntime,
    AudioRackSpec, ResolvedMacroAssignment, resolve_rack_macros_on_control,
};
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
    PreparedParameterAutomationPoint, curve_shape,
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
mod instrument_outputs;
mod instrument_pair;
mod instrument_rack;
mod instrument_runtime;
mod midi_processor;
mod midi_rack;
pub use instrument::{
    ANALYTIC_NOTE_PAYLOAD_WORDS, AnalyticNoteContinuation, MAX_SHARED_ANALYTIC_CONTEXT_BYTES,
    NativeInstrumentDefinition, NativeInstrumentRegistry, PreparedAnalyticNote,
    PreparedNativeInstrument, PreparedSharedAnalyticContext, PreparedSharedAnalyticNote,
    PreparedStorageDeclaration, SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS,
};
pub use instrument_gain::{
    INSTRUMENT_GAIN_UNITY, MAX_INSTRUMENT_OCCURRENCE_GAIN, NativeInstrumentOccurrenceGain,
};
pub use instrument_outputs::{
    MAX_NATIVE_OUTPUT_BLOCK_FRAMES, MAX_NATIVE_OUTPUT_COMPENSATION_DELAY_FRAMES,
    MAX_NATIVE_OUTPUT_DESTINATIONS, MAX_NATIVE_OUTPUT_FANOUT_PER_PORT,
    MAX_NATIVE_OUTPUT_GRAPH_EDGES, MAX_NATIVE_OUTPUT_PORTS, MAX_NATIVE_OUTPUT_ROUTE_GAIN,
    MAX_NATIVE_OUTPUT_ROUTES, MultiOutputDestinationBuffer, MultiOutputInstrumentRuntime,
    MultiOutputRoutingError, NATIVE_MULTI_OUTPUT_PLAN_VERSION, NativeGraphEdge,
    NativeMultiOutputEdit, NativeMultiOutputHistory, NativeMultiOutputLimits,
    NativeMultiOutputPlan, NativeMultiOutputRoute, NativeMultiOutputTopology,
    NativeOutputChannelLayout, NativeOutputDestination, NativeOutputDestinationInfo,
    NativeOutputPortDescriptor, NativeOutputPortId, PreparedMultiOutputRouter,
    validate_output_port_descriptors,
};
pub use instrument_pair::{
    NativeOwnedInstrumentRuntimePair, OwnedInstrumentRuntime, PreparedOwnedInstrumentRuntime,
};
pub use instrument_rack::{
    INSTRUMENT_RACK_BRANCH_COUNT, INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES,
    INSTRUMENT_RACK_MAX_BRANCH_GAIN, INSTRUMENT_RACK_MAX_COMPENSATION_DELAY_FRAMES,
    INSTRUMENT_RACK_MAX_DEPTH, INSTRUMENT_RACK_MAX_EVENTS_PER_BLOCK,
    INSTRUMENT_RACK_RUNTIME_VERSION, INSTRUMENT_RACK_STATE_VERSION, INSTRUMENT_RACK_TYPE_ID,
    InstrumentRackBranchSpec, InstrumentRackDefinition, InstrumentRackSpec, PARAM_BRANCH_1_BYPASS,
    PARAM_BRANCH_1_GAIN, PARAM_BRANCH_2_BYPASS, PARAM_BRANCH_2_GAIN,
};
pub use instrument_runtime::{
    AnalyticInstrumentHistory, InstrumentHistoryRetargetPreparation, InstrumentParameterRuntime,
    MAX_INSTRUMENT_AUTOMATION_FRAMES, MutableInstrumentRuntime, NativeInstrumentAnalyticSeed,
    NativeInstrumentClock, NativeInstrumentEvent, NativeInstrumentExpression,
    NativeInstrumentRuntimeError, NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
    PreparedInstrumentHistoryRetarget, PreparedMutableInstrumentRuntime,
};
pub use midi_processor::{
    BoundedFutureMidiQueue, DueMidiEvent, FUTURE_MIDI_EVENT_QUEUE_CAPACITY,
    FUTURE_MIDI_MAX_HORIZON_BEATS_Q32, FUTURE_MIDI_MAX_HORIZON_FRAMES,
    FUTURE_MIDI_RESERVED_NOTE_OFF_SLOTS, FutureMidiTime, GENERATED_OCCURRENCE_ID_BASE,
    MidiProcessorOutput, MutableMidiProcessorRuntime, NativeMidiProcessorDefinition,
    NativeMidiProcessorError, NativeMidiProcessorLimits, NativeMidiProcessorRegistry,
    NativeMidiProcessorRuntimePair, PreparedNativeMidiProcessor, ScheduledMidiEvent,
    ScheduledMidiKind, resolve_future_time_in_block, validate_future_horizon,
};
pub use midi_rack::{
    MIDI_RACK_BRANCH_COUNT, MIDI_RACK_MAX_ACTIVE_OCCURRENCES,
    MIDI_RACK_MAX_BRANCH_ACTIVE_OCCURRENCES, MIDI_RACK_MAX_DEPTH, MIDI_RACK_MAX_INPUT_EVENTS,
    MIDI_RACK_MAX_OUTPUT_EVENTS, MIDI_RACK_MAX_WORK_UNITS, MIDI_RACK_RUNTIME_VERSION,
    MIDI_RACK_STATE_VERSION, MIDI_RACK_TYPE_ID, MidiRackBranchSpec, MidiRackDefinition,
    MidiRackSpec, PARAM_MIDI_RACK_BRANCH_1_BYPASS, PARAM_MIDI_RACK_BRANCH_2_BYPASS,
    PreparedMidiRack,
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
