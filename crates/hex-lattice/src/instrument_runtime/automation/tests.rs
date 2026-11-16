use super::*;
use hex_lattice_contract::{ParameterId, ParameterKind, ParameterPresentation, ParameterValue};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const ID: ParameterId = ParameterId::from_static(1);
const DESCRIPTORS: &[ParameterDescriptor] = &[ParameterDescriptor {
    id: ID,
    label: "level",
    kind: ParameterKind::Float {
        min: 0.0,
        max: 4.0,
        unit: None,
    },
    default: ParameterValue::Float(1.0),
    presentation: ParameterPresentation::Linear,
}];
const LIMITS: NativeInstrumentRuntimeLimits = NativeInstrumentRuntimeLimits {
    max_pair_bytes: 4096,
    max_events_per_block: 8,
    max_active_occurrences: 2,
    max_tail_frames: 0,
};
struct Factory {
    grants: Vec<ParameterDescriptor>,
    runtime_grants: Option<&'static [ParameterDescriptor]>,
    creations: Arc<AtomicUsize>,
    readonly: [u8; 2],
    target_fault: Arc<AtomicUsize>,
}
impl Factory {
    fn supported() -> Self {
        Self {
            grants: DESCRIPTORS.to_vec(),
            runtime_grants: Some(DESCRIPTORS),
            creations: Arc::default(),
            readonly: [0; 2],
            target_fault: Arc::default(),
        }
    }
}
#[derive(Clone)]
struct Runtime {
    grants: Option<&'static [ParameterDescriptor]>,
    age: usize,
    level: f32,
    gated: bool,
    readonly: u8,
    target_fault: Arc<AtomicUsize>,
}
impl PreparedMutableInstrumentRuntime for Factory {
    fn parameter_automation_descriptors(&self) -> &[ParameterDescriptor] {
        &self.grants
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
    }
    fn max_events_per_block(&self) -> usize {
        8
    }
    fn max_active_occurrences(&self) -> usize {
        2
    }
    fn max_release_tail_frames(&self) -> u64 {
        0
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        let index = self.creations.fetch_add(1, Ordering::Relaxed);
        Box::new(Runtime {
            grants: self.runtime_grants,
            age: 0,
            level: 1.0,
            gated: false,
            readonly: self.readonly[index % 2],
            target_fault: Arc::clone(&self.target_fault),
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl MutableInstrumentRuntime for Runtime {
    fn parameter_automation_readonly(&self) -> Option<&dyn InstrumentParameterRuntime> {
        match self.readonly {
            0 => None,
            2 => Some(&WRONG_READONLY),
            _ => Some(self),
        }
    }
    fn parameter_automation(&mut self) -> Option<&mut dyn InstrumentParameterRuntime> {
        self.grants
            .map(|_| self as &mut dyn InstrumentParameterRuntime)
    }
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        _: usize,
        _: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if events
            .iter()
            .any(|event| matches!(event, NativeInstrumentEvent::NoteOn { pitch: 127, .. }))
        {
            Err(NativeInstrumentRuntimeError::EventCapacityExceeded)
        } else {
            Ok(())
        }
    }
    fn preflight_clock(
        &self,
        clock: Option<&dyn NativeInstrumentClock>,
        _: usize,
        _: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if clock.is_some_and(|clock| clock.beat_at_offset(0).is_none()) {
            Err(NativeInstrumentRuntimeError::ResourceUnavailable)
        } else {
            Ok(())
        }
    }
    fn process_block(
        &mut self,
        _: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _: &dyn crate::DeviceResources,
    ) {
        for frame in frames {
            frame[0] += 1.0;
            frame[1] += 1.0;
            self.age += 1;
        }
    }
    fn reset(&mut self) {
        self.age = 0;
        self.level = 1.0;
        self.gated = false;
    }
    fn is_quiescent(&self) -> bool {
        !self.gated
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.age = previous.age;
            self.level = previous.level;
            self.gated = previous.gated;
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
    fn copy_targets(
        &self,
        output: &mut [Option<ParameterValue>],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if self.readonly == 3 {
            return Err(NativeInstrumentRuntimeError::UnsupportedParameterAutomation);
        }
        match self.target_fault.load(Ordering::Relaxed) {
            1 => Err(NativeInstrumentRuntimeError::InvalidParameterAutomation),
            2 => Ok(()),
            3 => {
                output[0] = Some(ParameterValue::Bool(true));
                Ok(())
            }
            4 => {
                output[0] = Some(ParameterValue::Float(f32::NAN));
                Ok(())
            }
            5 => {
                output[0] = Some(ParameterValue::Float(2.0));
                Err(NativeInstrumentRuntimeError::InvalidParameterAutomation)
            }
            6 => {
                output.fill(Some(ParameterValue::Float(self.level)));
                if let Some(last) = output.last_mut() {
                    *last = Some(ParameterValue::Float(f32::NAN));
                }
                Ok(())
            }
            _ => {
                output.fill(Some(ParameterValue::Float(self.level)));
                Ok(())
            }
        }
    }
    fn descriptors(&self) -> &[ParameterDescriptor] {
        self.grants.unwrap_or_default()
    }
    fn preflight(
        &self,
        _: &[NativeInstrumentEvent],
        frames: usize,
        _: Option<&dyn NativeInstrumentClock>,
        _: &[crate::NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        _: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        // A provider-specific coupled law, beyond the descriptor's scalar range.
        if (0..frames).any(|offset| values.value_at(ID, offset) == Some(ParameterValue::Float(3.0)))
        {
            Err(NativeInstrumentRuntimeError::InvalidParameterAutomation)
        } else {
            Ok(())
        }
    }
    fn process(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        _: &[crate::NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        _: &dyn crate::DeviceResources,
    ) {
        for (offset, frame) in frames.iter_mut().enumerate() {
            self.level = match values.value_at(ID, offset) {
                Some(ParameterValue::Float(value)) => value,
                _ => 1.0,
            };
            for event in events.iter().filter(|event| event.frame_offset() == offset) {
                match event {
                    NativeInstrumentEvent::NoteOn { .. } => self.gated = true,
                    NativeInstrumentEvent::NoteOff { .. } => self.gated = false,
                    _ => {}
                }
            }
            if self.gated {
                frame[0] += self.level * (self.age + 1) as f32;
                frame[1] = frame[0];
            }
            self.age += 1;
        }
    }
}
struct Values {
    ids: Vec<ParameterId>,
    start: usize,
    bad: Option<usize>,
    base: bool,
    coupled: bool,
}
impl Values {
    fn authored() -> Self {
        Self {
            ids: vec![ID],
            start: 0,
            bad: None,
            base: false,
            coupled: false,
        }
    }
}
impl NativeParameterAutomation for Values {
    fn parameter_ids(&self) -> &[ParameterId] {
        &self.ids
    }
    fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
        if self.base || !self.ids.contains(&id) {
            return None;
        }
        Some(ParameterValue::Float(if self.bad == Some(offset) {
            f32::NAN
        } else if self.coupled {
            3.0
        } else {
            1.0 + (self.start + offset) as f32 / 64.0
        }))
    }
}
fn on(pitch: u8) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch,
        velocity: 127,
    }
}
fn run(
    pair: &mut NativeInstrumentRuntimePair,
    events: &[NativeInstrumentEvent],
    frames: &mut [StereoFrame],
    values: &Values,
) -> Result<(), NativeInstrumentRuntimeError> {
    pair.process_block_with_clock_and_automation(
        events,
        frames,
        None,
        &[],
        values,
        &crate::BorrowedDeviceResources(&[]),
    )
}
fn state(pair: &NativeInstrumentRuntimePair) -> (usize, f32, bool) {
    let runtime = pair.scratch.as_any().downcast_ref::<Runtime>().unwrap();
    (runtime.age, runtime.level, runtime.gated)
}
#[test]
fn invalid_grants_refuse_before_twin_creation() {
    for grants in [
        vec![DESCRIPTORS[0]; 2],
        vec![ParameterDescriptor {
            id: ParameterId::from_static(0),
            ..DESCRIPTORS[0]
        }],
        vec![DESCRIPTORS[0]; MAX_PARAMETERS_PER_DEVICE + 1],
    ] {
        let mut factory = Factory::supported();
        factory.grants = grants;
        assert!(matches!(
            NativeInstrumentRuntimePair::new(&factory, LIMITS),
            Err(NativeInstrumentRuntimeError::InvalidParameterAutomation)
        ));
        assert_eq!(factory.creations.load(Ordering::Relaxed), 0);
    }
}
#[test]
fn factory_and_actual_views_must_agree() {
    for (grants, runtime_grants) in [
        (DESCRIPTORS.to_vec(), None),
        (vec![], Some(DESCRIPTORS)),
        (
            vec![ParameterDescriptor {
                label: "other",
                ..DESCRIPTORS[0]
            }],
            Some(DESCRIPTORS),
        ),
    ] {
        let factory = Factory {
            grants,
            runtime_grants,
            creations: Arc::default(),
            readonly: [0; 2],
            target_fault: Arc::default(),
        };
        assert!(matches!(
            NativeInstrumentRuntimePair::new(&factory, LIMITS),
            Err(NativeInstrumentRuntimeError::IncompatibleTwins)
        ));
    }
}
#[test]
fn default_provider_refuses_values_but_keeps_ordinary_empty_path() {
    let factory = Factory {
        grants: vec![],
        runtime_grants: None,
        creations: Arc::default(),
        readonly: [0; 2],
        target_fault: Arc::default(),
    };
    let mut pair = NativeInstrumentRuntimePair::new(&factory, LIMITS).unwrap();
    assert!(!pair.supports_parameter_automation());
    pair.begin_block();
    let mut frames = [StereoFrame::default(); 2];
    assert_eq!(
        run(&mut pair, &[], &mut frames, &Values::authored()),
        Err(NativeInstrumentRuntimeError::UnsupportedParameterAutomation)
    );
    assert_eq!(state(&pair).0, 0);
    let mut values = Values::authored();
    values.ids.clear();
    run(&mut pair, &[], &mut frames, &values).unwrap();
    assert_eq!(frames[0][0], 1.0);
}
#[test]
fn malformed_late_values_and_provider_refusals_are_atomic() {
    let mut pair = NativeInstrumentRuntimePair::new(&Factory::supported(), LIMITS).unwrap();
    pair.begin_block();
    for values in [
        Values {
            bad: Some(7),
            ..Values::authored()
        },
        Values {
            ids: vec![ID, ID],
            ..Values::authored()
        },
        Values {
            ids: vec![ParameterId::from_static(2)],
            ..Values::authored()
        },
        Values {
            ids: (1..=MAX_PARAMETERS_PER_DEVICE as u32 + 1)
                .map(ParameterId::from_static)
                .collect(),
            ..Values::authored()
        },
        Values {
            coupled: true,
            ..Values::authored()
        },
    ] {
        let mut frames = [[7.0, 8.0]; 8];
        let before = frames;
        assert_eq!(
            run(&mut pair, &[on(60)], &mut frames, &values),
            Err(NativeInstrumentRuntimeError::InvalidParameterAutomation)
        );
        assert_eq!(frames, before);
        assert_eq!(state(&pair), (0, 1.0, false));
    }
}
#[test]
fn authored_base_empty_and_rollback_preserve_voice_history() {
    let mut pair = NativeInstrumentRuntimePair::new(&Factory::supported(), LIMITS).unwrap();
    let mut first = [StereoFrame::default(); 4];
    assert_eq!(
        run(&mut pair, &[], &mut first, &Values::authored()),
        Err(NativeInstrumentRuntimeError::BlockNotStarted)
    );
    pair.begin_block();
    run(&mut pair, &[on(60)], &mut first, &Values::authored()).unwrap();
    let saved = state(&pair);
    run(&mut pair, &[], &mut [], &Values::authored()).unwrap();
    assert_eq!(state(&pair), saved);
    pair.rollback_block();
    pair.begin_block();
    let mut retry = [StereoFrame::default(); 4];
    run(&mut pair, &[on(60)], &mut retry, &Values::authored()).unwrap();
    assert_eq!(retry, first);
    pair.commit_block();
    for empty_ids in [false, true] {
        pair.begin_block();
        let mut values = Values {
            base: true,
            ..Values::authored()
        };
        if empty_ids {
            values.ids.clear();
        }
        let mut frames = [StereoFrame::default(); 1];
        run(&mut pair, &[], &mut frames, &values).unwrap();
        assert_eq!(frames[0][0], 5.0);
        assert_eq!(state(&pair), (5, 1.0, true));
        pair.rollback_block();
    }
}
#[test]
fn segment_partition_uses_original_offsets_without_restarting_notes() {
    let factory = Factory::supported();
    let mut whole = NativeInstrumentRuntimePair::new(&factory, LIMITS).unwrap();
    let mut split = NativeInstrumentRuntimePair::new(&factory, LIMITS).unwrap();
    whole.begin_block();
    split.begin_block();
    let mut expected = [StereoFrame::default(); 12];
    let mut actual = expected;
    run(&mut whole, &[on(60)], &mut expected, &Values::authored()).unwrap();
    run(&mut split, &[on(60)], &mut actual[..5], &Values::authored()).unwrap();
    run(
        &mut split,
        &[],
        &mut actual[5..],
        &Values {
            start: 5,
            ..Values::authored()
        },
    )
    .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(state(&whole), state(&split));
}
#[test]
fn frame_and_event_bounds_refuse_without_mutation() {
    let mut pair = NativeInstrumentRuntimePair::new(&Factory::supported(), LIMITS).unwrap();
    pair.begin_block();
    let mut frames = vec![StereoFrame::default(); MAX_INSTRUMENT_AUTOMATION_FRAMES + 1];
    assert_eq!(
        run(&mut pair, &[], &mut frames, &Values::authored()),
        Err(NativeInstrumentRuntimeError::InvalidParameterAutomation)
    );
    assert_eq!(
        run(&mut pair, &[on(127)], &mut frames[..1], &Values::authored()),
        Err(NativeInstrumentRuntimeError::EventCapacityExceeded)
    );
    let invalid = NativeInstrumentEvent::NoteOff {
        frame_offset: 1,
        occurrence_id: 1,
    };
    assert_eq!(
        run(&mut pair, &[invalid], &mut frames[..1], &Values::authored()),
        Err(NativeInstrumentRuntimeError::InvalidEventBatch)
    );
    assert_eq!(state(&pair), (0, 1.0, false));
    assert!(frames.iter().all(|frame| *frame == StereoFrame::default()));
}

struct MissingClock;
impl NativeInstrumentClock for MissingClock {
    fn beat_at_offset(&self, _: usize) -> Option<u64> {
        None
    }
}
struct BadBulk;
impl NativeParameterAutomation for BadBulk {
    fn parameter_ids(&self) -> &[ParameterId] {
        &[ID]
    }
    fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
        Some(ParameterValue::Float(1.0))
    }
    fn values_at(
        &self,
        _: usize,
        _: &mut [Option<ParameterValue>],
    ) -> Result<(), DeviceContractError> {
        Err(DeviceContractError::InvalidParameterValue)
    }
}
#[test]
fn clock_gain_and_bulk_refusals_leave_pcm_and_history_untouched() {
    let mut pair = NativeInstrumentRuntimePair::new(&Factory::supported(), LIMITS).unwrap();
    pair.begin_block();
    let mut frames = [[7.0, 8.0]; 2];
    let before = frames;
    let resources = crate::BorrowedDeviceResources(&[]);
    assert_eq!(
        pair.process_block_with_clock_and_automation(
            &[on(60)],
            &mut frames,
            Some(&MissingClock),
            &[],
            &Values::authored(),
            &resources
        ),
        Err(NativeInstrumentRuntimeError::ResourceUnavailable)
    );
    let gain = crate::NativeInstrumentOccurrenceGain::new(1, 1.0, 1.0, 2, 0).unwrap();
    assert_eq!(
        pair.process_block_with_clock_and_automation(
            &[on(60)],
            &mut frames,
            None,
            &[gain],
            &Values::authored(),
            &resources
        ),
        Err(NativeInstrumentRuntimeError::UnsupportedOccurrenceGain)
    );
    assert_eq!(
        pair.process_block_with_clock_and_automation(
            &[on(60)],
            &mut frames,
            None,
            &[],
            &BadBulk,
            &resources
        ),
        Err(NativeInstrumentRuntimeError::InvalidParameterAutomation)
    );
    assert_eq!(frames, before);
    assert_eq!(state(&pair), (0, 1.0, false));
}

struct WrongReadonly;
static WRONG_READONLY: WrongReadonly = WrongReadonly;
impl InstrumentParameterRuntime for WrongReadonly {
    fn descriptors(&self) -> &[ParameterDescriptor] {
        static OTHER: [ParameterDescriptor; 1] = [ParameterDescriptor {
            id: ParameterId::from_static(2),
            ..DESCRIPTORS[0]
        }];
        &OTHER
    }
    fn preflight(
        &self,
        _: &[NativeInstrumentEvent],
        _: usize,
        _: Option<&dyn NativeInstrumentClock>,
        _: &[crate::NativeInstrumentOccurrenceGain],
        _: &dyn NativeParameterAutomation,
        _: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        Ok(())
    }
    fn process(
        &mut self,
        _: &[NativeInstrumentEvent],
        _: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        _: &[crate::NativeInstrumentOccurrenceGain],
        _: &dyn NativeParameterAutomation,
        _: &dyn crate::DeviceResources,
    ) {
    }
}
fn snapshot_factory() -> Factory {
    Factory {
        readonly: [1; 2],
        ..Factory::supported()
    }
}
#[test]
fn target_snapshot_opt_in_requires_both_exact_complete_views() {
    let pair = NativeInstrumentRuntimePair::new(&Factory::supported(), LIMITS).unwrap();
    assert!(!pair.supports_committed_parameter_targets());
    let original = [Some(ParameterValue::Float(0.25))];
    let mut output = original;
    assert!(
        pair.copy_committed_parameter_targets(&[ID], &mut output)
            .is_err()
    );
    assert_eq!(output, original);
    for readonly in [[0, 1], [1, 0], [2, 2], [3, 3]] {
        let factory = Factory {
            readonly,
            ..Factory::supported()
        };
        assert!(NativeInstrumentRuntimePair::new(&factory, LIMITS).is_err());
    }
    for fault in 1..=5 {
        let factory = snapshot_factory();
        factory.target_fault.store(fault, Ordering::Relaxed);
        assert!(NativeInstrumentRuntimePair::new(&factory, LIMITS).is_err());
    }
}
#[test]
fn target_snapshot_refusal_preserves_caller_output_for_bad_grants_and_provider_values() {
    let factory = snapshot_factory();
    let pair = NativeInstrumentRuntimePair::new(&factory, LIMITS).unwrap();
    assert!(pair.supports_committed_parameter_targets());
    let original = [Some(ParameterValue::Float(0.25))];
    for ids in [vec![], vec![ParameterId::from_static(2)], vec![ID, ID]] {
        let mut output = original;
        assert!(
            pair.copy_committed_parameter_targets(&ids, &mut output)
                .is_err()
        );
        assert_eq!(output, original);
    }
    let mut long = [Some(ParameterValue::Float(0.25)); 2];
    assert!(
        pair.copy_committed_parameter_targets(&[ID], &mut long)
            .is_err()
    );
    assert_eq!(long, [Some(ParameterValue::Float(0.25)); 2]);
    for fault in 1..=5 {
        factory.target_fault.store(fault, Ordering::Relaxed);
        let mut output = original;
        assert!(
            pair.copy_committed_parameter_targets(&[ID], &mut output)
                .is_err()
        );
        assert_eq!(output, original);
    }
}
#[test]
fn target_snapshot_observes_only_committed_controls_across_rollback_base_and_reset() {
    let mut pair = NativeInstrumentRuntimePair::new(&snapshot_factory(), LIMITS).unwrap();
    let read = |pair: &NativeInstrumentRuntimePair| {
        let mut output = [None];
        pair.copy_committed_parameter_targets(&[ID], &mut output)
            .unwrap();
        output[0]
    };
    let base = Some(ParameterValue::Float(1.0));
    assert_eq!(read(&pair), base);
    pair.begin_block();
    run(
        &mut pair,
        &[on(60)],
        &mut [[0.0; 2]; 4],
        &Values::authored(),
    )
    .unwrap();
    assert_eq!(read(&pair), base, "speculative controls stay invisible");
    pair.rollback_block();
    assert_eq!(read(&pair), base);
    pair.begin_block();
    run(
        &mut pair,
        &[on(60)],
        &mut [[0.0; 2]; 4],
        &Values::authored(),
    )
    .unwrap();
    pair.commit_block();
    let authored = Some(ParameterValue::Float(1.0 + 3.0 / 64.0));
    assert_eq!(read(&pair), authored);
    pair.begin_block();
    run(
        &mut pair,
        &[],
        &mut [[0.0; 2]; 1],
        &Values {
            base: true,
            ..Values::authored()
        },
    )
    .unwrap();
    assert_eq!(read(&pair), authored);
    pair.commit_block();
    assert_eq!(read(&pair), base);
    pair.begin_block();
    run(&mut pair, &[], &mut [[0.0; 2]; 4], &Values::authored()).unwrap();
    pair.commit_block();
    pair.reset();
    assert_eq!(read(&pair), base);
}

#[test]
fn late_invalid_target_never_publishes_an_earlier_valid_prefix() {
    static TWO: [ParameterDescriptor; 2] = [
        DESCRIPTORS[0],
        ParameterDescriptor {
            id: ParameterId::from_static(2),
            ..DESCRIPTORS[0]
        },
    ];
    let factory = Factory {
        grants: TWO.to_vec(),
        runtime_grants: Some(&TWO),
        ..snapshot_factory()
    };
    let pair = NativeInstrumentRuntimePair::new(&factory, LIMITS).unwrap();
    let ids = [ID, ParameterId::from_static(2)];
    let original = [Some(ParameterValue::Float(0.25)); 2];
    let mut output = original;
    assert!(
        pair.copy_committed_parameter_targets(&[ids[1], ids[0]], &mut output)
            .is_err()
    );
    assert_eq!(output, original);
    factory.target_fault.store(6, Ordering::Relaxed);
    assert!(
        pair.copy_committed_parameter_targets(&ids, &mut output)
            .is_err()
    );
    assert_eq!(output, original);
}

#[test]
fn automation_only_view_refuses_an_active_sample_rate_route_before_rendering() {
    struct Routed;
    impl NativeParameterAutomation for Routed {
        fn parameter_ids(&self) -> &[ParameterId] {
            &[ID]
        }
        fn has_sample_rate_modulation(&self, parameter: ParameterId) -> bool {
            parameter == ID
        }
        fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
            Some(ParameterValue::Float(1.5))
        }
    }

    let mut pair = NativeInstrumentRuntimePair::new(&Factory::supported(), LIMITS).unwrap();
    let mut output = [[0.0; 2]; 4];
    pair.begin_block();
    assert_eq!(
        pair.process_block_with_clock_and_automation(
            &[on(60)],
            &mut output,
            None,
            &[],
            &Routed,
            &crate::EMPTY_DEVICE_RESOURCES,
        ),
        Err(NativeInstrumentRuntimeError::UnsupportedParameterModulation),
    );
    assert_eq!(output, [[0.0; 2]; 4]);
    pair.process_block_with_clock_and_automation(
        &[on(60)],
        &mut output,
        None,
        &[],
        &Values::authored(),
        &crate::EMPTY_DEVICE_RESOURCES,
    )
    .unwrap();
    assert!(output[0][0] > 0.0);
    pair.commit_block();
}
