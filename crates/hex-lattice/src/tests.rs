use super::*;
use hex_lattice_contract::{ParameterDescriptor, ParameterId, ParameterValue};

#[test]
fn source_processor_identity_is_nonzero_and_independent_of_track_and_return() {
    let source = ProcessorInstanceIdentity::new(ProcessorChainOwner::Source(4), 7).unwrap();
    assert_eq!(source.owner(), ProcessorChainOwner::Source(4));
    assert_ne!(
        source,
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(4), 7).unwrap()
    );
    assert_ne!(
        source,
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Return(4), 7).unwrap()
    );
    assert!(ProcessorInstanceIdentity::new(ProcessorChainOwner::Source(0), 7).is_err());
}
const DYNAMICS_METER_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.dynamics-meter");

struct DynamicsMeterPrepared;
struct DynamicsMeterRuntime {
    pending: DynamicsMeter,
}

impl PreparedNativeProcessor for DynamicsMeterPrepared {
    fn runtime_version(&self) -> u32 {
        1
    }

    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<DynamicsMeterRuntime>()
    }

    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(DynamicsMeterRuntime {
            pending: DynamicsMeter::default(),
        })
    }
}

impl NativeProcessorRuntime for DynamicsMeterRuntime {
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
        self.pending = DynamicsMeter {
            reduction_db: [
                frames
                    .iter()
                    .map(|frame| frame[0].abs())
                    .fold(0.0, f32::max),
                frames
                    .iter()
                    .map(|frame| frame[1].abs())
                    .fold(0.0, f32::max),
            ],
            clipped: frames.iter().any(|frame| frame[0].abs() > 1.0),
            post_input_peak: frames
                .iter()
                .flatten()
                .map(|sample| sample.abs())
                .fold(0.0, f32::max),
        };
    }

    fn reset(&mut self) {
        self.pending = DynamicsMeter::default();
    }

    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }

    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.pending = previous.pending;
        }
    }

    fn as_any(&self) -> &dyn StdAny {
        self
    }

    fn take_dynamics_meter(&mut self) -> Option<DynamicsMeter> {
        Some(std::mem::take(&mut self.pending))
    }
}

fn dynamics_meter_chain(bypassed: bool) -> NativeRuntimeChain {
    NativeRuntimeChain::new(vec![PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(3),
        device_id: 9,
        device_type: DYNAMICS_METER_TYPE,
        matrix: StereoMatrix::identity(),
        bypassed,
        sample_rate: 48_000,
        configuration: None,
        definition: Some(std::sync::Arc::new(DynamicsMeterPrepared)),
    }])
    .expect("prepared dynamics meter runtime")
}

#[test]
fn dynamics_meter_publishes_only_the_committed_active_runtime_reading() {
    let mut chain = dynamics_meter_chain(false);
    chain.set_metering(true);

    chain.begin_block();
    let mut speculative = [[2.5, 4.0]];
    chain.process_block(&mut speculative).unwrap();
    chain.rollback_block();
    assert_eq!(
        chain.entries[0].meter.take_dynamics(),
        DynamicsMeter::default()
    );
    // The input tap combines both channels; the output tap keeps left and
    // right apart. Neither depends on committed provider dynamics readings.
    assert_eq!(
        chain.entries[0].meter.take_peaks(),
        crate::DevicePeaks {
            input: 4.0,
            output: [2.5, 4.0],
        }
    );

    chain.begin_block();
    let mut committed = [[0.25, 0.75]];
    chain.process_block(&mut committed).unwrap();
    chain.commit_block();

    assert_eq!(
        chain.entries[0].meter.take_dynamics(),
        DynamicsMeter {
            reduction_db: [0.25, 0.75],
            clipped: false,
            post_input_peak: 0.75,
        }
    );
    assert_eq!(
        chain.entries[0].meter.take_peaks(),
        crate::DevicePeaks {
            input: 0.75,
            output: [0.25, 0.75],
        }
    );
}

#[test]
fn dynamics_meter_drains_readings_while_metering_is_disabled_or_device_bypassed() {
    let mut disabled = dynamics_meter_chain(false);
    disabled.begin_block();
    let mut frames = [[2.0, 3.0]];
    disabled.process_block(&mut frames).unwrap();
    disabled.commit_block();
    assert_eq!(
        disabled.entries[0].meter.take_dynamics(),
        DynamicsMeter::default()
    );

    disabled.set_metering(true);
    disabled.begin_block();
    let mut quiet = [[0.0, 0.0]];
    disabled.process_block(&mut quiet).unwrap();
    disabled.commit_block();
    assert_eq!(
        disabled.entries[0].meter.take_dynamics(),
        DynamicsMeter::default()
    );

    let mut bypassed = dynamics_meter_chain(true);
    bypassed.set_metering(true);
    bypassed.begin_block();
    let mut bypassed_frames = [[2.0, 3.0]];
    bypassed.process_block(&mut bypassed_frames).unwrap();
    bypassed.commit_block();
    assert_eq!(
        bypassed.entries[0].meter.take_dynamics(),
        DynamicsMeter::default()
    );
    assert_eq!(
        bypassed.entries[0].meter.take_peaks(),
        crate::DevicePeaks {
            input: 3.0,
            output: [2.0, 3.0],
        }
    );
}

const SNAPSHOT_PATCH_PARAMETER: ParameterId = ParameterId::from_static(1);
const SNAPSHOT_PATCH_DESCRIPTORS: &[ParameterDescriptor] = &[ParameterDescriptor {
    id: SNAPSHOT_PATCH_PARAMETER,
    label: "level",
    kind: hex_lattice_contract::ParameterKind::Float {
        min: 0.0,
        max: 1.0,
        unit: None,
    },
    default: ParameterValue::Float(0.25),
    presentation: hex_lattice_contract::ParameterPresentation::Linear,
}];
const SNAPSHOT_PATCH_INPUT_GAIN: i32 = -12_345;
const SNAPSHOT_PATCH_OUTPUT_GAIN: i32 = 6_789;

struct SnapshotPatchEffectDefinition;

impl NativeDeviceMetadata for SnapshotPatchEffectDefinition {
    fn type_id(&self) -> DeviceTypeId {
        DeviceTypeId::from_static("test.effect-snapshot-patch")
    }

    fn state_version(&self) -> u32 {
        1
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        SNAPSHOT_PATCH_DESCRIPTORS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(NativeDeviceMetadata::type_id(self), 1, vec![64])
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != NativeDeviceMetadata::type_id(self) || state.state_version() != 1 {
            return Err(DeviceContractError::MalformedState);
        }
        NativeDeviceState::new(
            NativeDeviceMetadata::type_id(self),
            1,
            state.payload().to_vec(),
        )
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        if parameter != SNAPSHOT_PATCH_PARAMETER {
            return Err(DeviceContractError::UnknownParameter);
        }
        let value = *state
            .payload()
            .first()
            .ok_or(DeviceContractError::MalformedState)?;
        Ok(ParameterValue::Float(f32::from(value) / 255.0))
    }

    fn patch_parameter(
        &self,
        _state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if parameter != SNAPSHOT_PATCH_PARAMETER {
            return Err(DeviceContractError::UnknownParameter);
        }
        let ParameterValue::Float(value) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };
        NativeDeviceState::new(
            NativeDeviceMetadata::type_id(self),
            1,
            vec![(value * 255.0).round() as u8],
        )
    }

    fn patch_parameters(
        &self,
        state: &NativeDeviceState,
        snapshot: &hex_lattice_contract::DeviceParameterSnapshot,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.input_gain_millidb() != SNAPSHOT_PATCH_INPUT_GAIN
            || state.output_gain_millidb() != SNAPSHOT_PATCH_OUTPUT_GAIN
        {
            return Err(DeviceContractError::MalformedState);
        }
        let mut patched = self.migrate_state(state)?;
        for &(parameter, value) in snapshot.entries() {
            patched = self.patch_parameter(&patched, parameter, value)?;
        }
        Ok(patched)
    }
}

impl NativeDeviceDefinition for SnapshotPatchEffectDefinition {
    fn prepare_matrix(
        &self,
        _state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Ok(StereoMatrix::identity())
    }
}

#[test]
fn effect_snapshot_patch_preserves_host_gains_across_migrations() {
    let definition = SnapshotPatchEffectDefinition;
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let state = registry
        .default_state(NativeDeviceMetadata::type_id(&definition))
        .unwrap()
        .with_input_gain_millidb(SNAPSHOT_PATCH_INPUT_GAIN)
        .unwrap()
        .with_output_gain_millidb(SNAPSHOT_PATCH_OUTPUT_GAIN)
        .unwrap()
        .with_asset_references(vec![hex_lattice_contract::DeviceAssetReference {
            slot: hex_lattice_contract::DeviceAssetSlot::new(1).unwrap(),
            asset: hex_lattice_contract::DeviceAssetKey::new(42).unwrap(),
        }])
        .unwrap();
    let snapshot = hex_lattice_contract::DeviceParameterSnapshot::new(&[(
        SNAPSHOT_PATCH_PARAMETER,
        ParameterValue::Float(1.0),
    )])
    .unwrap();

    let patched = registry.patch_parameters(&state, &snapshot).unwrap();

    assert_eq!(patched.input_gain_millidb(), SNAPSHOT_PATCH_INPUT_GAIN);
    assert_eq!(patched.output_gain_millidb(), SNAPSHOT_PATCH_OUTPUT_GAIN);
    assert_eq!(patched.asset_references(), state.asset_references());
    assert_eq!(
        definition
            .read_parameter(&patched, SNAPSHOT_PATCH_PARAMETER)
            .unwrap(),
        ParameterValue::Float(1.0)
    );
}

struct SplitAuditionPrepared(std::sync::atomic::AtomicUsize);

struct SplitAuditionRuntime {
    accepts: bool,
    requested: Option<ProcessorAuditionSelection>,
}

impl PreparedNativeProcessor for SplitAuditionPrepared {
    fn runtime_version(&self) -> u32 {
        1
    }

    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<SplitAuditionRuntime>()
    }

    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        let index = self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Box::new(SplitAuditionRuntime {
            accepts: index == 0,
            requested: None,
        })
    }
}

impl NativeProcessorRuntime for SplitAuditionRuntime {
    fn process_block(
        &mut self,
        _frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
    }

    fn reset(&mut self) {
        self.requested = None;
    }

    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }

    fn copy_history_from(&mut self, _previous: &dyn NativeProcessorRuntime) {}

    fn as_any(&self) -> &dyn StdAny {
        self
    }

    fn transient_audition(&mut self) -> Option<&mut dyn NativeProcessorAudition> {
        Some(self)
    }
}

impl NativeProcessorAudition for SplitAuditionRuntime {
    fn can_select(&self, _selection: ProcessorAuditionSelection) -> bool {
        self.accepts
    }

    fn requested_selection(&self) -> Option<ProcessorAuditionSelection> {
        self.requested
    }

    fn select(&mut self, selection: Option<ProcessorAuditionSelection>) {
        self.requested = selection;
    }

    fn cancel_immediately(&mut self) {
        self.requested = None;
    }
}

#[test]
fn audition_preflights_both_twins_before_mutating_either() {
    use std::sync::atomic::AtomicUsize;

    let device_type = DeviceTypeId::from_static("test.split-audition");
    let definition: std::sync::Arc<dyn PreparedNativeProcessor> =
        std::sync::Arc::new(SplitAuditionPrepared(AtomicUsize::new(0)));
    let mut chain = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(3),
        device_id: 9,
        device_type,
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: None,
        definition: Some(definition),
    }])
    .expect("prepared twins");
    let selection = ProcessorAuditionSelection { index: 2, lane: 0 };

    assert!(!chain.select_processor_audition(
        ProcessorChainOwner::Track(3),
        9,
        device_type,
        Some(selection),
    ));
    let entry = &mut chain.entries[0];
    let (primary, scratch) = entry.runtime.legacy_mut().expect("legacy runtimes");
    let primary_audition = primary.transient_audition().expect("primary capability");
    assert_eq!(primary_audition.requested_selection(), None);
    let scratch_audition = scratch.transient_audition().expect("scratch capability");
    assert_eq!(scratch_audition.requested_selection(), None);
}

struct IndependentDefinition;
struct IndependentPrepared(u32);
#[derive(Default)]
struct IndependentRuntime {
    history: f32,
}
struct IncompatibleRuntime;

impl NativeDeviceMetadata for IndependentDefinition {
    fn type_id(&self) -> DeviceTypeId {
        DeviceTypeId::from_static("test.independent")
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(NativeDeviceMetadata::type_id(self), 1, vec![1])
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != NativeDeviceMetadata::type_id(self)
            || state.state_version() != 1
            || state.payload().len() != 1
        {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(state.clone())
    }
    fn read_parameter(
        &self,
        _state: &NativeDeviceState,
        _parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
    fn patch_parameter(
        &self,
        _state: &NativeDeviceState,
        _parameter: ParameterId,
        _value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
}
impl NativeDeviceDefinition for IndependentDefinition {
    fn prepare_matrix(
        &self,
        _state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        _context: &crate::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        if state.payload() != [1] {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(Some(Box::new(IndependentPrepared(1))))
    }
}
impl PreparedNativeProcessor for IndependentPrepared {
    fn runtime_version(&self) -> u32 {
        self.0
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<IndependentRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(IndependentRuntime::default())
    }
}
impl NativeProcessorRuntime for IndependentRuntime {
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
        for frame in frames {
            frame[0] += self.history;
            frame[1] += self.history;
            self.history = frame[0];
        }
    }
    fn reset(&mut self) {
        self.history = 0.0;
    }
    fn is_quiescent(&self) -> bool {
        self.history == 0.0
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.history = previous.history;
        }
    }
    fn as_any(&self) -> &dyn StdAny {
        self
    }
}

struct DelayedTailPrepared;

impl PreparedNativeProcessor for DelayedTailPrepared {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<DelayedTailRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(DelayedTailRuntime::default())
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 3 }
    }
}

#[derive(Default)]
struct DelayedTailRuntime {
    delay: [StereoFrame; 3],
}

impl NativeProcessorRuntime for DelayedTailRuntime {
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
        for frame in frames {
            let delayed = self.delay[0];
            self.delay.rotate_left(1);
            self.delay[2] = *frame;
            *frame = delayed;
        }
    }
    fn reset(&mut self) {
        self.delay = [[0.0; 2]; 3];
    }
    fn is_quiescent(&self) -> bool {
        self.delay.iter().all(|frame| *frame == [0.0; 2])
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.delay = previous.delay;
        }
    }
    fn as_any(&self) -> &dyn StdAny {
        self
    }
}

#[test]
fn finite_output_tail_keeps_delayed_residual_until_bound_and_reset() {
    let prepared = DelayedTailPrepared;
    assert_eq!(
        prepared.output_tail(),
        OutputTail::Finite { max_tail_frames: 3 }
    );
    for partition in [1, 2, 7] {
        let mut runtime = prepared.create_runtime();
        assert!(runtime.is_quiescent());
        let mut impulse = [[1.0, -0.5]];
        runtime.process_block(&mut impulse, &crate::EMPTY_DEVICE_RESOURCES);
        assert_eq!(impulse, [[0.0, 0.0]]);
        assert!(!runtime.is_quiescent());

        let mut silence = [[0.0, 0.0]; 3];
        let mut processed = 0;
        for block in silence.chunks_mut(partition) {
            runtime.process_block(block, &crate::EMPTY_DEVICE_RESOURCES);
            processed += block.len();
            assert_eq!(runtime.is_quiescent(), processed == 3);
        }
        assert_eq!(silence, [[0.0, 0.0], [0.0, 0.0], [1.0, -0.5]]);
        assert!(runtime.is_quiescent());

        runtime.reset();
        assert!(runtime.is_quiescent());
    }
}

#[test]
fn chain_combines_tail_ranges_and_queries_committed_and_speculative_quiescence() {
    let finite: std::sync::Arc<dyn PreparedNativeProcessor> =
        std::sync::Arc::new(DelayedTailPrepared);
    let unbounded: std::sync::Arc<dyn PreparedNativeProcessor> =
        std::sync::Arc::new(IndependentPrepared(1));
    let instance = |device_id, definition| PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(3),
        device_id,
        device_type: DeviceTypeId::from_static("test.tail"),
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: None,
        definition: Some(definition),
    };
    let matrix_only = PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(3),
        device_id: 1,
        device_type: DeviceTypeId::from_static("test.matrix"),
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: None,
        definition: None,
    };
    let mut chain = NativeRuntimeChain::new(vec![
        matrix_only,
        instance(2, std::sync::Arc::clone(&finite)),
        instance(3, std::sync::Arc::clone(&finite)),
    ])
    .unwrap();
    assert!(!chain.select_processor_audition(
        ProcessorChainOwner::Track(3),
        1,
        DeviceTypeId::from_static("test.matrix"),
        Some(ProcessorAuditionSelection { index: 0, lane: 0 }),
    ));
    assert_eq!(
        chain.output_tail_for_range(0..3),
        Some(OutputTail::Finite { max_tail_frames: 6 })
    );
    assert_eq!(
        chain.output_tail_for_range(1..2),
        Some(OutputTail::Finite { max_tail_frames: 3 })
    );
    assert_eq!(
        chain.output_tail_for_range(3..3),
        Some(OutputTail::Finite { max_tail_frames: 0 })
    );
    assert_eq!(chain.output_tail_for_range(0..4), None);
    assert_eq!(
        chain.output_tail_for_range(std::ops::Range { start: 2, end: 1 }),
        None
    );
    assert!(chain.committed_range_is_quiescent(1..3));
    chain.begin_block();
    let mut impulse = [[1.0, -1.0]];
    chain.process_range(1..3, &mut impulse).unwrap();
    assert!(!chain.speculative_range_is_quiescent(1..3));
    assert!(chain.committed_range_is_quiescent(1..3));
    chain.commit_block();
    assert!(!chain.committed_range_is_quiescent(1..3));

    let unbounded_chain = NativeRuntimeChain::new(vec![instance(4, unbounded)]).unwrap();
    assert_eq!(
        unbounded_chain.output_tail_for_range(0..1),
        Some(OutputTail::Unbounded)
    );
    assert_eq!(
        unbounded_chain.output_tail_for_range(0..0),
        Some(OutputTail::Finite { max_tail_frames: 0 })
    );
}

#[test]
fn configuration_range_requires_exact_shared_state_and_prepared_semantics() {
    let device_type = DeviceTypeId::from_static("test.config");
    let configuration =
        || std::sync::Arc::new(NativeDeviceState::new(device_type, 1, vec![1, 2, 3]).unwrap());
    let instance = |configuration, bypassed| PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(3),
        device_id: 9,
        device_type,
        matrix: StereoMatrix::identity(),
        bypassed,
        sample_rate: 48_000,
        configuration,
        definition: None,
    };
    let old = NativeRuntimeChain::new(vec![instance(Some(configuration()), false)]).unwrap();
    let same_content =
        NativeRuntimeChain::new(vec![instance(Some(configuration()), false)]).unwrap();
    let changed = NativeRuntimeChain::new(vec![instance(
        Some(std::sync::Arc::new(
            NativeDeviceState::new(device_type, 1, vec![1, 2, 4]).unwrap(),
        )),
        false,
    )])
    .unwrap();
    let missing = NativeRuntimeChain::new(vec![instance(None, false)]).unwrap();
    let bypassed = NativeRuntimeChain::new(vec![instance(Some(configuration()), true)]).unwrap();

    assert!(old.configuration_matches_range(&same_content, 0..1));
    assert!(!old.configuration_matches_range(&changed, 0..1));
    assert!(!old.configuration_matches_range(&missing, 0..1));
    assert!(
        old.configuration_matches_range(&bypassed, 0..1),
        "zero-latency bypass retains its live chain"
    );
    assert!(!old.configuration_matches_range(&same_content, 0..2));
    assert!(!old.configuration_matches_range(&same_content, std::ops::Range { start: 1, end: 0 }));
}

struct RetargetPrepared {
    allow: bool,
    scratch_allow: bool,
    version: u32,
    runtime_count: std::sync::atomic::AtomicUsize,
}

#[derive(Default)]
struct RetargetRuntime {
    allow: bool,
}

impl PreparedNativeProcessor for RetargetPrepared {
    fn runtime_version(&self) -> u32 {
        self.version
    }

    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<RetargetRuntime>()
    }

    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        let twin = self
            .runtime_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Box::new(RetargetRuntime {
            allow: if twin == 0 {
                self.allow
            } else {
                self.scratch_allow
            },
        })
    }
}

impl NativeProcessorRuntime for RetargetRuntime {
    fn process_block(
        &mut self,
        _frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
    }
    fn reset(&mut self) {}
    fn is_quiescent(&self) -> bool {
        true
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, _previous: &dyn NativeProcessorRuntime) {}
    fn can_retarget_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        self.allow && previous.as_any().is::<Self>()
    }
    fn as_any(&self) -> &dyn StdAny {
        self
    }
}

struct BypassRetargetPrepared {
    smooth: bool,
    retarget: bool,
    latency: u32,
}

impl PreparedNativeProcessor for BypassRetargetPrepared {
    fn supports_smoothed_bypass(&self) -> bool {
        self.smooth
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<RetargetRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(RetargetRuntime {
            allow: self.retarget,
        })
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.latency)
    }
}

#[test]
fn native_insert_refuses_a_changed_retained_order_or_configuration() {
    let kind = DeviceTypeId::from_static("test.native-insert");
    let chain = |entries: &[(u64, u8)]| {
        NativeRuntimeChain::new(
            entries
                .iter()
                .map(|&(device_id, payload)| {
                    let configuration = std::sync::Arc::new(
                        NativeDeviceState::new(kind, 1, vec![payload]).unwrap(),
                    );
                    PreparedProcessorInstance {
                        meter: Default::default(),
                        owner: ProcessorChainOwner::Track(1),
                        device_id,
                        device_type: kind,
                        matrix: StereoMatrix::identity(),
                        bypassed: false,
                        sample_rate: 48_000,
                        configuration: Some(configuration),
                        definition: Some(std::sync::Arc::new(BypassRetargetPrepared {
                            smooth: true,
                            retarget: true,
                            latency: 0,
                        })),
                    }
                })
                .collect(),
        )
        .unwrap()
    };
    let old = chain(&[(1, 1), (2, 2)]);
    assert!(
        chain(&[(1, 1), (3, 3), (2, 2)])
            .native_insert_transition_from(&old, None)
            .is_some()
    );
    assert!(
        chain(&[(2, 2), (3, 3), (1, 1)])
            .native_insert_transition_from(&old, None)
            .is_none()
    );
    assert!(
        chain(&[(1, 1), (3, 3), (2, 4)])
            .native_insert_transition_from(&old, None)
            .is_none()
    );
}

#[test]
fn combined_bypass_retarget_requires_both_capabilities_and_stable_host_contract() {
    let kind = DeviceTypeId::from_static("test.bypass-retarget");
    let chain = |payload, bypassed, smooth, retarget, latency, gain| {
        let state = NativeDeviceState::new(kind, 1, vec![payload])
            .unwrap()
            .with_output_gain_db(gain)
            .unwrap();
        NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(1),
            device_id: 3,
            device_type: kind,
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 48_000,
            configuration: Some(std::sync::Arc::new(state)),
            definition: Some(std::sync::Arc::new(BypassRetargetPrepared {
                smooth,
                retarget,
                latency,
            })),
        }])
        .unwrap()
    };
    let old = chain(1, false, true, true, 0, 0.0);
    let mut unsupported = chain(1, true, true, true, 0, 0.0);
    assert!(!unsupported.can_fork_timing_without_signal_history_at(&old, 0));
    assert!(
        unsupported
            .fork_timing_without_signal_history_at(&old, 0)
            .is_err()
    );
    let mut missing_configuration = chain(2, true, true, true, 0, 0.0);
    missing_configuration.entries[0].configuration = None;
    assert!(!missing_configuration.can_retarget_parameters_from_range(&old, 0..1));
    assert!(chain(2, true, true, true, 0, 0.0).can_retarget_parameters_from_range(&old, 0..1));
    let old_without_opt_in = chain(1, false, false, true, 0, 0.0);
    assert!(
        !chain(2, true, true, true, 0, 0.0)
            .can_retarget_parameters_from_range(&old_without_opt_in, 0..1)
    );
    assert!(!chain(2, true, false, true, 0, 0.0).can_retarget_parameters_from_range(&old, 0..1));
    assert!(!chain(2, true, true, false, 0, 0.0).can_retarget_parameters_from_range(&old, 0..1));
    assert!(!chain(2, true, true, true, 1, 0.0).can_retarget_parameters_from_range(&old, 0..1));
    assert!(!chain(2, true, true, true, 0, 6.0).can_retarget_parameters_from_range(&old, 0..1));
}

#[test]
fn parameter_retarget_requires_stable_chain_topology_and_candidate_twin_opt_in() {
    let device_type = DeviceTypeId::from_static("test.parameter-retarget");
    let instance = |payload: u8,
                    allow: bool,
                    scratch_allow: bool,
                    owner,
                    device_id,
                    device_type,
                    version,
                    bypassed,
                    matrix,
                    sample_rate| {
        let configuration =
            std::sync::Arc::new(NativeDeviceState::new(device_type, 1, vec![payload]).unwrap());
        let definition: std::sync::Arc<dyn PreparedNativeProcessor> =
            std::sync::Arc::new(RetargetPrepared {
                allow,
                scratch_allow,
                version,
                runtime_count: std::sync::atomic::AtomicUsize::new(0),
            });
        PreparedProcessorInstance {
            meter: Default::default(),
            owner,
            device_id,
            device_type,
            matrix,
            bypassed,
            sample_rate,
            configuration: Some(configuration),
            definition: Some(definition),
        }
    };
    let old = NativeRuntimeChain::new(vec![instance(
        1,
        false,
        false,
        ProcessorChainOwner::Track(3),
        9,
        device_type,
        1,
        false,
        StereoMatrix::identity(),
        48_000,
    )])
    .unwrap();
    let candidate = NativeRuntimeChain::new(vec![instance(
        2,
        true,
        true,
        ProcessorChainOwner::Track(3),
        9,
        device_type,
        1,
        false,
        StereoMatrix::identity(),
        48_000,
    )])
    .unwrap();
    assert!(candidate.can_retarget_parameters_from_range(&old, 0..1));
    assert!(!candidate.configuration_matches_range(&old, 0..1));

    let one_twin_opted_in = NativeRuntimeChain::new(vec![instance(
        2,
        true,
        false,
        ProcessorChainOwner::Track(3),
        9,
        device_type,
        1,
        false,
        StereoMatrix::identity(),
        48_000,
    )])
    .unwrap();
    assert!(!one_twin_opted_in.can_retarget_parameters_from_range(&old, 0..1));

    let no_opt_in = NativeRuntimeChain::new(vec![instance(
        2,
        false,
        false,
        ProcessorChainOwner::Track(3),
        9,
        device_type,
        1,
        false,
        StereoMatrix::identity(),
        48_000,
    )])
    .unwrap();
    assert!(!no_opt_in.can_retarget_parameters_from_range(&old, 0..1));

    let bypassed = NativeRuntimeChain::new(vec![instance(
        2,
        true,
        true,
        ProcessorChainOwner::Track(3),
        9,
        device_type,
        1,
        true,
        StereoMatrix::identity(),
        48_000,
    )])
    .unwrap();
    assert!(!bypassed.can_retarget_parameters_from_range(&old, 0..1));

    let wrong_rate = NativeRuntimeChain::new(vec![instance(
        2,
        true,
        true,
        ProcessorChainOwner::Track(3),
        9,
        device_type,
        1,
        false,
        StereoMatrix::identity(),
        44_100,
    )])
    .unwrap();
    assert!(!wrong_rate.can_retarget_parameters_from_range(&old, 0..1));
    assert!(!candidate.can_retarget_parameters_from_range(&old, 0..0));
    assert!(!candidate.can_retarget_parameters_from_range(&old, 0..2));

    let wrong_owner = NativeRuntimeChain::new(vec![instance(
        2,
        true,
        true,
        ProcessorChainOwner::Return(3),
        9,
        device_type,
        1,
        false,
        StereoMatrix::identity(),
        48_000,
    )])
    .unwrap();
    assert!(!wrong_owner.can_retarget_parameters_from_range(&old, 0..1));

    let wrong_id = NativeRuntimeChain::new(vec![instance(
        2,
        true,
        true,
        ProcessorChainOwner::Track(3),
        10,
        device_type,
        1,
        false,
        StereoMatrix::identity(),
        48_000,
    )])
    .unwrap();
    assert!(!wrong_id.can_retarget_parameters_from_range(&old, 0..1));

    let other_type = DeviceTypeId::from_static("test.other-parameter-retarget");
    let wrong_type = NativeRuntimeChain::new(vec![instance(
        2,
        true,
        true,
        ProcessorChainOwner::Track(3),
        9,
        other_type,
        1,
        false,
        StereoMatrix::identity(),
        48_000,
    )])
    .unwrap();
    assert!(!wrong_type.can_retarget_parameters_from_range(&old, 0..1));

    let wrong_version = NativeRuntimeChain::new(vec![instance(
        2,
        true,
        true,
        ProcessorChainOwner::Track(3),
        9,
        device_type,
        2,
        false,
        StereoMatrix::identity(),
        48_000,
    )])
    .unwrap();
    assert!(!wrong_version.can_retarget_parameters_from_range(&old, 0..1));

    let wrong_matrix = NativeRuntimeChain::new(vec![instance(
        2,
        true,
        true,
        ProcessorChainOwner::Track(3),
        9,
        device_type,
        1,
        false,
        StereoMatrix::new([[0.5, 0.0], [0.0, 1.0]]).unwrap(),
        48_000,
    )])
    .unwrap();
    assert!(!wrong_matrix.can_retarget_parameters_from_range(&old, 0..1));
}

impl NativeProcessorRuntime for IncompatibleRuntime {
    fn process_block(
        &mut self,
        _frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
    }
    fn reset(&mut self) {}
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, _previous: &dyn NativeProcessorRuntime) {}
    fn as_any(&self) -> &dyn StdAny {
        self
    }
}

#[test]
fn independently_registered_stateful_definition_uses_paired_transactional_runtime() {
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&IndependentDefinition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let prepared = registry
        .prepare_processor(
            &registry
                .default_state(DeviceTypeId::from_static("test.independent"))
                .unwrap(),
            &crate::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &crate::TuningTable::default(),
                resources: &crate::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(prepared.algorithmic_latency_frames(), None);
    assert_eq!(prepared.output_tail(), OutputTail::Unbounded);
    let legacy_runtime: Box<dyn NativeProcessorRuntime> = Box::new(IncompatibleRuntime);
    assert!(!legacy_runtime.is_quiescent());
    let mut feedback = prepared.create_runtime();
    assert!(feedback.is_quiescent());
    let mut excitation = [[1.0, 1.0]];
    feedback.process_block(&mut excitation, &crate::EMPTY_DEVICE_RESOURCES);
    assert!(!feedback.is_quiescent());
    for _ in 0..32 {
        let mut silence = [[0.0, 0.0]; 7];
        feedback.process_block(&mut silence, &crate::EMPTY_DEVICE_RESOURCES);
        assert!(silence.iter().all(|frame| *frame != [0.0, 0.0]));
        assert!(!feedback.is_quiescent());
    }
    feedback.reset();
    assert!(feedback.is_quiescent());
    let mut chain = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(3),
        device_id: 9,
        device_type: DeviceTypeId::from_static("test.independent"),
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: None,
        definition: Some(std::sync::Arc::from(prepared)),
    }])
    .unwrap();
    let mut committed = [[1.0, 1.0]];
    chain.begin_block();
    chain.process_block(&mut committed).unwrap();
    chain.commit_block();

    chain.begin_block();
    let mut failed_block = [[2.0, 2.0]];
    chain.process_block(&mut failed_block).unwrap();
    // No commit models a source/render failure after this processor ran.
    chain.begin_block();
    let mut retry = [[2.0, 2.0]];
    chain.process_block(&mut retry).unwrap();
    chain.commit_block();
    assert_eq!(retry, [[3.0, 3.0]]);

    let prepared = registry
        .prepare_processor(
            &registry
                .default_state(DeviceTypeId::from_static("test.independent"))
                .unwrap(),
            &crate::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &crate::TuningTable::default(),
                resources: &crate::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    let mut successor = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(3),
        device_id: 9,
        device_type: DeviceTypeId::from_static("test.independent"),
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: None,
        definition: Some(std::sync::Arc::from(prepared)),
    }])
    .unwrap();
    assert!(successor.can_inherit_from(&chain));
    successor.inherit_from(&chain);
    successor.begin_block();
    let mut inherited = [[1.0, 1.0]];
    successor.process_block(&mut inherited).unwrap();
    assert_eq!(inherited, [[4.0, 4.0]]);

    let prepared = registry
        .prepare_processor(
            &registry
                .default_state(DeviceTypeId::from_static("test.independent"))
                .unwrap(),
            &crate::DevicePreparationContext {
                sample_rate: 44_100,
                tuning: &crate::TuningTable::default(),
                resources: &crate::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    let mut incompatible_rate = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(3),
        device_id: 9,
        device_type: DeviceTypeId::from_static("test.independent"),
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 44_100,
        configuration: None,
        definition: Some(std::sync::Arc::from(prepared)),
    }])
    .unwrap();
    assert!(incompatible_rate.can_inherit_from(&chain));
    incompatible_rate.inherit_from(&chain);
    incompatible_rate.begin_block();
    let mut reset_history = [[1.0, 1.0]];
    incompatible_rate.process_block(&mut reset_history).unwrap();
    assert_eq!(reset_history, [[1.0, 1.0]]);
}

#[test]
fn selective_inheritance_excludes_old_suffix_by_identity_after_reorder() {
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&IndependentDefinition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let prepared = registry
        .prepare_processor(
            &registry
                .default_state(DeviceTypeId::from_static("test.independent"))
                .unwrap(),
            &crate::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &crate::TuningTable::default(),
                resources: &crate::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    let prepared: std::sync::Arc<dyn PreparedNativeProcessor> = std::sync::Arc::from(prepared);
    let instance = |device_id| PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(3),
        device_id,
        device_type: DeviceTypeId::from_static("test.independent"),
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: None,
        definition: Some(std::sync::Arc::clone(&prepared)),
    };
    let mut previous = NativeRuntimeChain::new(vec![instance(9), instance(10)]).unwrap();
    previous.begin_block();
    let mut old_signal = [[1.0, 1.0]];
    previous.process_block(&mut old_signal).unwrap();
    previous.commit_block();
    assert_eq!(old_signal, [[1.0, 1.0]]);

    // Device 10 is the retained old suffix but moves to index 0 in the
    // candidate; device 9 is an inherited processor despite moving to 1.
    let mut candidate = NativeRuntimeChain::new(vec![instance(10), instance(9)]).unwrap();
    assert!(!candidate.can_inherit_from_excluding_old_range(&previous, 1..3));
    assert!(candidate.can_inherit_from_excluding_old_range(&previous, 1..2));
    candidate
        .inherit_from_excluding_old_range(&previous, 1..2)
        .unwrap();
    candidate.begin_block();
    let mut fresh_audio = [[0.0, 0.0]];
    candidate.process_block(&mut fresh_audio).unwrap();
    // The new suffix starts fresh (device 10 contributes zero); device 9
    // retains its prefix history and contributes one.
    assert_eq!(fresh_audio, [[1.0, 1.0]]);
}

#[test]
fn full_chain_preflight_rejects_an_incompatible_retained_suffix() {
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&IndependentDefinition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let prepared = registry
        .prepare_processor(
            &registry
                .default_state(DeviceTypeId::from_static("test.independent"))
                .unwrap(),
            &crate::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &crate::TuningTable::default(),
                resources: &crate::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    let prepared: std::sync::Arc<dyn PreparedNativeProcessor> = std::sync::Arc::from(prepared);
    let instance = || PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(3),
        device_id: 10,
        device_type: DeviceTypeId::from_static("test.independent"),
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: None,
        definition: Some(std::sync::Arc::clone(&prepared)),
    };
    let previous = NativeRuntimeChain::new(vec![instance()]).unwrap();
    let mut candidate = NativeRuntimeChain::new(vec![instance()]).unwrap();

    // Model a factory/runtime compatibility change behind unchanged
    // identity metadata. Excluding this old suffix is valid for a tail
    // owner, but exact full-chain carry must refuse before inherit_from.
    let entry = candidate.entries.first_mut().unwrap();
    entry.runtime = RuntimeOwnership::Legacy {
        primary: Box::new(IncompatibleRuntime),
        scratch: Box::new(IncompatibleRuntime),
    };
    assert!(candidate.can_inherit_from_excluding_old_range(&previous, 0..1));
    assert!(!candidate.can_inherit_from(&previous));
}

#[test]
fn segmented_processing_shares_one_transaction_and_rolls_back_all_ranges() {
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&IndependentDefinition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let prepared = registry
        .prepare_processor(
            &registry
                .default_state(DeviceTypeId::from_static("test.independent"))
                .unwrap(),
            &crate::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &crate::TuningTable::default(),
                resources: &crate::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    let prepared: std::sync::Arc<dyn PreparedNativeProcessor> = std::sync::Arc::from(prepared);
    let make_instance = |device_id| PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(3),
        device_id,
        device_type: DeviceTypeId::from_static("test.independent"),
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: None,
        definition: Some(std::sync::Arc::clone(&prepared)),
    };
    let mut chain = NativeRuntimeChain::new(vec![make_instance(9), make_instance(10)]).unwrap();

    chain.begin_block();
    let mut first = [[1.0, 1.0]];
    chain.process_range(0..1, &mut first).unwrap();
    chain.process_range(1..2, &mut first).unwrap();
    chain.commit_block();

    chain.begin_block();
    let mut speculative = [[2.0, 2.0]];
    chain.process_range(0..1, &mut speculative).unwrap();
    chain.process_range(1..2, &mut speculative).unwrap();
    // A later render-stage failure leaves both segment histories uncommitted.
    chain.begin_block();
    let mut retry = [[2.0, 2.0]];
    chain.process_range(0..1, &mut retry).unwrap();
    chain.process_range(1..2, &mut retry).unwrap();
    chain.commit_block();
    assert_eq!(retry, [[4.0, 4.0]]);

    chain.begin_block();
    let mut next = [[3.0, 3.0]];
    chain.process_range(0..1, &mut next).unwrap();
    chain.process_range(1..2, &mut next).unwrap();
    chain.commit_block();
    assert_eq!(next, [[10.0, 10.0]]);

    chain.begin_block();
    let mut untouched = [[7.0, 7.0]];
    assert_eq!(
        chain.process_range(0..3, &mut untouched),
        Err(DeviceContractError::InvalidDescriptor)
    );
    assert_eq!(untouched, [[7.0, 7.0]]);
}

#[test]
fn bypass_advances_history_after_matrix_without_changing_dry_pcm() {
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&IndependentDefinition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let prepared = registry
        .prepare_processor(
            &registry
                .default_state(DeviceTypeId::from_static("test.independent"))
                .unwrap(),
            &crate::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &crate::TuningTable::default(),
                resources: &crate::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    let prepared: std::sync::Arc<dyn PreparedNativeProcessor> = std::sync::Arc::from(prepared);
    let matrix = StereoMatrix::new([[2.0, 0.0], [0.0, 2.0]]).unwrap();
    let make_chain = |bypassed| {
        NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(3),
            device_id: 9,
            device_type: DeviceTypeId::from_static("test.independent"),
            matrix,
            bypassed,
            sample_rate: 48_000,
            configuration: None,
            definition: Some(std::sync::Arc::clone(&prepared)),
        }])
        .unwrap()
    };
    let mut bypassed = make_chain(true);
    let mut active_reference = make_chain(false);
    let warmup = [[0.25, -0.125], [0.5, 0.75], [-0.25, 0.375]];
    let mut dry = warmup;

    bypassed.begin_block();
    // Different chunking must not change the state advanced while bypassed.
    bypassed.process_range(0..1, &mut dry[..1]).unwrap();
    bypassed.process_block(&mut dry[1..]).unwrap();
    bypassed.commit_block();

    active_reference.begin_block();
    let mut audible_reference = warmup;
    active_reference
        .process_block(&mut audible_reference)
        .unwrap();
    active_reference.commit_block();

    for (actual, original) in dry.iter().zip(warmup) {
        assert_eq!(
            [actual[0].to_bits(), actual[1].to_bits()],
            [original[0].to_bits(), original[1].to_bits()],
            "bypass must preserve the original samples exactly"
        );
    }

    // Re-enable both processors and compare their output. Their hidden
    // histories must match the active reference despite bypass chunking.
    bypassed.entries[0].bypassed = false;
    let resumed = [[0.125, -0.125], [0.0, 0.25]];
    let mut resumed_bypassed = resumed;
    let mut resumed_reference = resumed;
    bypassed.begin_block();
    bypassed
        .process_range(0..1, &mut resumed_bypassed[..1])
        .unwrap();
    bypassed.process_block(&mut resumed_bypassed[1..]).unwrap();
    bypassed.commit_block();
    active_reference.begin_block();
    active_reference
        .process_block(&mut resumed_reference)
        .unwrap();
    active_reference.commit_block();

    assert_eq!(resumed_bypassed, resumed_reference);
}

#[test]
fn source_neutral_append_requires_exact_prefix_and_fresh_identity() {
    let device_type = DeviceTypeId::from_static("test.source-matrix");
    let entry = |source, id, gain: f32, config: u8| PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Source(source),
        device_id: id,
        device_type,
        matrix: StereoMatrix::new([[gain, 0.0], [0.0, gain]]).unwrap(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: Some(std::sync::Arc::new(
            NativeDeviceState::new(device_type, 1, vec![config]).unwrap(),
        )),
        definition: None,
    };
    let old = NativeRuntimeChain::new(vec![entry(1, 4, 1.0, 1)]).unwrap();
    let candidate =
        NativeRuntimeChain::new(vec![entry(1, 4, 1.0, 1), entry(1, 5, 1.0, 2)]).unwrap();
    assert!(candidate.can_transfer_source_recipe_history_from(&old, true));
    assert!(!candidate.can_transfer_source_recipe_history_from(&old, false));
    for bad in [
        vec![entry(1, 4, 1.0, 1), entry(1, 5, 2.0, 2)],
        vec![entry(1, 4, 1.0, 1), entry(1, 4, 1.0, 2)],
        vec![entry(1, 4, 1.0, 1), entry(2, 5, 1.0, 2)],
        vec![entry(1, 4, 1.0, 2), entry(1, 5, 1.0, 2)],
        vec![entry(1, 5, 1.0, 2), entry(1, 4, 1.0, 1)],
    ] {
        let candidate = NativeRuntimeChain::new(bad).unwrap();
        assert!(!candidate.can_transfer_source_recipe_history_from(&old, true));
    }
    let mixed_owner =
        NativeRuntimeChain::new(vec![entry(1, 4, 1.0, 1), entry(2, 6, 1.0, 3)]).unwrap();
    let mixed_owner_candidate = NativeRuntimeChain::new(vec![
        entry(1, 4, 1.0, 1),
        entry(2, 6, 1.0, 3),
        entry(1, 5, 1.0, 2),
    ])
    .unwrap();
    assert!(!mixed_owner_candidate.can_transfer_source_recipe_history_from(&mixed_owner, true));
    let mut other_rate = entry(1, 6, 1.0, 3);
    other_rate.sample_rate = 44_100;
    let mixed_rate = NativeRuntimeChain::new(vec![entry(1, 4, 1.0, 1), other_rate]).unwrap();
    let mut other_rate_candidate = entry(1, 6, 1.0, 3);
    other_rate_candidate.sample_rate = 44_100;
    let mixed_rate_candidate = NativeRuntimeChain::new(vec![
        entry(1, 4, 1.0, 1),
        other_rate_candidate,
        entry(1, 5, 1.0, 2),
    ])
    .unwrap();
    assert!(!mixed_rate_candidate.can_transfer_source_recipe_history_from(&mixed_rate, true));
    let mut wrong_rate = entry(1, 5, 1.0, 2);
    wrong_rate.sample_rate = 44_100;
    let wrong_rate = NativeRuntimeChain::new(vec![entry(1, 4, 1.0, 1), wrong_rate]).unwrap();
    assert!(!wrong_rate.can_transfer_source_recipe_history_from(&old, true));
    let mut automated =
        NativeRuntimeChain::new(vec![entry(1, 4, 1.0, 1), entry(1, 5, 1.0, 2)]).unwrap();
    automated.entries[1]
        .automation
        .arm_host_gain_input(hex_lattice_contract::DeviceAutomationTarget::HostOutputGain)
        .unwrap();
    assert!(!automated.can_transfer_source_recipe_history_from(&old, true));
    let mut stateful = entry(1, 5, 1.0, 2);
    stateful.definition = Some(std::sync::Arc::new(DelayedTailPrepared));
    let stateful = NativeRuntimeChain::new(vec![entry(1, 4, 1.0, 1), stateful]).unwrap();
    assert!(!stateful.can_transfer_source_recipe_history_from(&old, true));
    let mut transient =
        NativeRuntimeChain::new(vec![entry(1, 4, 1.0, 1), entry(1, 5, 1.0, 2)]).unwrap();
    transient.entries[1].committed_transient_output_gain =
        host_output_gain_ramp::TransientGain::Releasing;
    assert!(!transient.can_transfer_source_recipe_history_from(&old, true));
    let empty = NativeRuntimeChain::new(vec![]).unwrap();
    assert!(!old.can_transfer_source_recipe_history_from(&empty, true));
}

#[test]
fn source_matrix_retarget_requires_matching_known_memoryless_identity() {
    let device_type = DeviceTypeId::from_static("test.source-matrix");
    let make = |source, id, rate, gain: f32, config: Option<u8>, bypassed, definition| {
        NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Source(source),
            device_id: id,
            device_type,
            matrix: StereoMatrix::new([[gain, 0.0], [0.0, gain]]).unwrap(),
            bypassed,
            sample_rate: rate,
            configuration: config.map(|n| {
                std::sync::Arc::new(NativeDeviceState::new(device_type, 1, vec![n]).unwrap())
            }),
            definition,
        }])
        .unwrap()
    };
    let old = make(1, 4, 48_000, 1.0, Some(1), false, None);
    let candidate = make(1, 4, 48_000, 2.0, Some(2), false, None);
    assert!(candidate.can_transfer_source_recipe_history_from(&old, true));
    assert!(!candidate.can_transfer_source_recipe_history_from(&old, false));
    let equivalent = make(1, 4, 48_000, 1.0, Some(2), false, None);
    assert!(equivalent.can_transfer_source_recipe_history_from(&old, true));
    assert!(!equivalent.can_transfer_source_recipe_history_from(&old, false));
    for bad in [
        make(2, 4, 48_000, 1.0, Some(2), false, None),
        make(1, 5, 48_000, 1.0, Some(2), false, None),
        make(1, 4, 48_000, 1.0, None, false, None),
        make(2, 4, 48_000, 2.0, Some(2), false, None),
        make(1, 5, 48_000, 2.0, Some(2), false, None),
        make(1, 4, 44_100, 2.0, Some(2), false, None),
        make(1, 4, 48_000, 2.0, None, false, None),
        make(1, 4, 48_000, 2.0, Some(2), true, None),
    ] {
        assert!(!bad.can_transfer_source_recipe_history_from(&old, true));
    }
    let unknown_old = make(1, 4, 48_000, 1.0, None, false, None);
    assert!(!candidate.can_transfer_source_recipe_history_from(&unknown_old, true));
    let mut automated = make(1, 4, 48_000, 2.0, Some(2), false, None);
    automated.entries[0]
        .automation
        .arm_host_gain_input(hex_lattice_contract::DeviceAutomationTarget::HostOutputGain)
        .unwrap();
    assert!(!automated.can_transfer_source_recipe_history_from(&old, true));
    automated.entries[0].matrix = old.entries[0].matrix;
    assert!(!automated.can_transfer_source_recipe_history_from(&old, true));
    let factory: std::sync::Arc<dyn PreparedNativeProcessor> =
        std::sync::Arc::new(DelayedTailPrepared);
    let stateful_old = make(
        1,
        4,
        48_000,
        1.0,
        Some(1),
        false,
        Some(std::sync::Arc::clone(&factory)),
    );
    let stateful_new = make(1, 4, 48_000, 2.0, Some(2), false, Some(factory));
    assert!(!stateful_new.can_transfer_source_recipe_history_from(&stateful_old, true));
    let mut equivalent_stateful = stateful_new;
    equivalent_stateful.entries[0].matrix = stateful_old.entries[0].matrix;
    assert!(!equivalent_stateful.can_transfer_source_recipe_history_from(&stateful_old, true));
}

struct SourceMatrixWitness(bool);
impl NativeProcessorRuntime for SourceMatrixWitness {
    fn process_block(
        &mut self,
        _frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
    }
    fn reset(&mut self) {}
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, _previous: &dyn NativeProcessorRuntime) {}
    fn can_retarget_source_host_matrix_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        self.0 && previous.as_any().is::<Self>()
    }
    fn can_retarget_source_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        self.0 && previous.as_any().is::<Self>()
    }
    fn as_any(&self) -> &dyn StdAny {
        self
    }
}

#[test]
fn source_matrix_provider_proof_requires_both_candidate_twins() {
    let legacy = |primary, scratch| RuntimeOwnership::Legacy {
        primary: Box::new(SourceMatrixWitness(primary)),
        scratch: Box::new(SourceMatrixWitness(scratch)),
    };
    let old = legacy(true, true);
    assert!(legacy(true, true).can_retarget_source_host_matrix_from(&old));
    assert!(!legacy(false, true).can_retarget_source_host_matrix_from(&old));
    assert!(!legacy(true, false).can_retarget_source_host_matrix_from(&old));
    assert!(!RuntimeOwnership::Memoryless.can_retarget_source_host_matrix_from(&old));
    assert!(legacy(true, true).can_retarget_source_parameters_from(&old));
    assert!(!legacy(false, true).can_retarget_source_parameters_from(&old));
    assert!(!legacy(true, false).can_retarget_source_parameters_from(&old));
    assert!(!RuntimeOwnership::Memoryless.can_retarget_source_parameters_from(&old));
}

struct SourceParameterPrepared;
impl PreparedNativeProcessor for SourceParameterPrepared {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<SourceParameterRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(SourceParameterRuntime)
    }
}
struct SourceParameterRuntime;
impl NativeProcessorRuntime for SourceParameterRuntime {
    fn process_block(&mut self, _: &mut [StereoFrame], _: &dyn crate::DeviceResources) {}
    fn reset(&mut self) {}
    fn can_copy_history_from(&self, old: &dyn NativeProcessorRuntime) -> bool {
        old.as_any().is::<Self>()
    }
    fn can_retarget_parameters_from(&self, old: &dyn NativeProcessorRuntime) -> bool {
        old.as_any().is::<Self>()
    }
    fn can_retarget_source_parameters_from(&self, old: &dyn NativeProcessorRuntime) -> bool {
        old.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, _: &dyn NativeProcessorRuntime) {}
    fn as_any(&self) -> &dyn StdAny {
        self
    }
}

#[test]
fn source_parameter_proof_requires_same_identity_matrix_and_no_automation() {
    let device_type = DeviceTypeId::from_static("test.source-parameter");
    let make = |source, id, rate, gain: f32, config: Option<u8>, bypassed| {
        NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Source(source),
            device_id: id,
            device_type,
            matrix: StereoMatrix::new([[gain, 0.0], [0.0, gain]]).unwrap(),
            bypassed,
            sample_rate: rate,
            configuration: config.map(|value| {
                std::sync::Arc::new(NativeDeviceState::new(device_type, 1, vec![value]).unwrap())
            }),
            definition: Some(std::sync::Arc::new(SourceParameterPrepared)),
        }])
        .unwrap()
    };
    let old = make(1, 4, 48_000, 1.0, Some(1), false);
    let candidate = make(1, 4, 48_000, 1.0, Some(2), false);
    assert!(candidate.can_transfer_source_recipe_history_from(&old, true));
    assert!(!candidate.can_transfer_source_recipe_history_from(&old, false));
    for bad in [
        make(2, 4, 48_000, 1.0, Some(2), false),
        make(1, 5, 48_000, 1.0, Some(2), false),
        make(1, 4, 44_100, 1.0, Some(2), false),
        make(1, 4, 48_000, 2.0, Some(2), false),
        make(1, 4, 48_000, 1.0, None, false),
        make(1, 4, 48_000, 1.0, Some(2), true),
    ] {
        assert!(!bad.can_transfer_source_recipe_history_from(&old, true));
    }
    let mut automated = make(1, 4, 48_000, 1.0, Some(2), false);
    automated.entries[0]
        .automation
        .arm_host_gain_input(hex_lattice_contract::DeviceAutomationTarget::HostOutputGain)
        .unwrap();
    assert!(!automated.can_transfer_source_recipe_history_from(&old, true));
}

#[test]
fn memoryless_native_insert_transition_ramps_dry_to_wet() {
    let stateful_type = DeviceTypeId::from_static("test.bypass-retarget");
    let utility_type = DeviceTypeId::from_static("test.memoryless-utility");
    let stateful = |device_id| {
        let configuration =
            std::sync::Arc::new(NativeDeviceState::new(stateful_type, 1, vec![1]).unwrap());
        PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(1),
            device_id,
            device_type: stateful_type,
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: Some(configuration),
            definition: Some(std::sync::Arc::new(BypassRetargetPrepared {
                smooth: true,
                retarget: true,
                latency: 0,
            })),
        }
    };
    let memoryless = |device_id| {
        let configuration =
            std::sync::Arc::new(NativeDeviceState::new(utility_type, 1, vec![2]).unwrap());
        PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(1),
            device_id,
            device_type: utility_type,
            matrix: StereoMatrix::new([[0.5, 0.0], [0.0, 0.5]]).unwrap(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: Some(configuration),
            definition: None,
        }
    };

    let mut old = NativeRuntimeChain::new(vec![stateful(1), stateful(2)]).unwrap();
    let mut candidate =
        NativeRuntimeChain::new(vec![stateful(1), memoryless(3), stateful(2)]).unwrap();
    let transition = candidate
        .native_insert_transition_from(&old, None)
        .expect("memoryless zero-latency insert must support smooth transition");
    candidate.transfer_history_with_native_insert_from(&mut old, transition);

    // Ramp begins dry (1.0) and settles to wet (0.5) without stepping.
    let mut frames = [[1.0_f32; 2]; 512];
    candidate.process_block(&mut frames).unwrap();
    candidate.commit_block();
    assert!((frames[0][0] - 1.0).abs() < 0.05);
    assert!((frames[511][0] - 0.5).abs() < 1e-6);
}
