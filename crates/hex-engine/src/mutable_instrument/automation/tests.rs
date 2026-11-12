use super::*;
use hex_lattice::{NativeInstrumentOccurrenceGain, NativeInstrumentRuntimePair};
use hex_lattice_contract::ParameterId;
use hex_project::{Command, Created, Editor, Project};

const IDS: [ParameterId; 4] = [
    ParameterId::from_static(29),
    ParameterId::from_static(30),
    ParameterId::from_static(31),
    ParameterId::from_static(32),
];
struct Values;
impl NativeParameterAutomation for Values {
    fn parameter_ids(&self) -> &[ParameterId] {
        &IDS
    }
    fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
        Some(ParameterValue::Float(
            ((offset / 53 + id.value() as usize) % 3) as f32 * 0.4 - 0.4,
        ))
    }
}
struct Base;
impl NativeParameterAutomation for Base {
    fn parameter_ids(&self) -> &[ParameterId] {
        &[]
    }
    fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
        None
    }
}
struct InvalidValues;
impl NativeParameterAutomation for InvalidValues {
    fn parameter_ids(&self) -> &[ParameterId] {
        &IDS
    }
    fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
        Some(ParameterValue::Float(f32::NAN))
    }
}
fn factory() -> Box<dyn PreparedMutableInstrumentRuntime> {
    let registry = hex_devices::native_instrument_registry().unwrap();
    let tuning = hex_lattice::TuningTable::default();
    registry
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
        .unwrap()
        .prepare_mutable_runtime()
        .unwrap()
        .unwrap()
}
fn onsets() -> MutableOnsetBuffer {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "voice".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let note = editor.project().midi_clip(clip).unwrap().notes()[0].id();
    let mut result = MutableOnsetBuffer::default();
    for (iteration, start) in [(0, 0), (1, 37)] {
        result
            .push(MutableNoteOnset {
                key: MutableOccurrenceKey::new(track, clip, note, iteration, 0),
                onset_beat: BeatTime::ZERO,
                gate_end_beat: BeatTime::ONE,
                source_start: start,
                source_gate_end: 48000,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    result
}
fn volume(
    _: MutableOccurrenceSignature,
    at: u64,
    _: usize,
) -> Result<Option<VolumeSlice>, MutableInstrumentRuntimeError> {
    Ok(Some(VolumeSlice {
        at,
        ramp: crate::midi_clip_volume::VolumeRamp {
            start_frame: 0,
            end_frame: u64::MAX,
            start_gain: 0.5,
            end_gain: 0.5,
            next_boundary: Some(at + 31 - at % 31),
        },
        terminal: None,
    }))
}
fn gain(id: u64) -> NativeInstrumentOccurrenceGain {
    NativeInstrumentOccurrenceGain::new(id, 0.5, 0.5, u64::MAX, 0).unwrap()
}

#[test]
fn real_fm4_streams_follow_volume_subdivisions_partitions_and_base_restoration() {
    let factory = factory();
    let mut oracle =
        NativeInstrumentRuntimePair::new(factory.as_ref(), runtime_limits(48000).unwrap()).unwrap();
    let mut expected = [[0.0; 2]; 512];
    oracle.begin_block();
    oracle
        .process_block_with_clock_and_automation(
            &[
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 60,
                    velocity: 100,
                },
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 37,
                    occurrence_id: 2,
                    pitch: 60,
                    velocity: 100,
                },
            ],
            &mut expected,
            None,
            &[gain(1), gain(2)],
            &Values,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    oracle.commit_block();
    let mut base_expected = [[0.0; 2]; 64];
    oracle.begin_block();
    oracle
        .process_block_with_clock_and_automation(
            &[],
            &mut base_expected,
            None,
            &[gain(1), gain(2)],
            &Base,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    oracle.commit_block();
    for partitions in [&[512][..], &[7, 13, 67][..]] {
        let mut runtime = MutableInstrumentTrackRuntime::new(factory.as_ref(), 48000).unwrap();
        let mut onsets = onsets();
        let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
        let mut actual = [[0.0; 2]; 512];
        let mut offset = 0;
        for &requested in partitions.iter().cycle() {
            if offset == actual.len() {
                break;
            }
            let count = requested.min(actual.len() - offset);
            let values = OffsetValues {
                values: &Values,
                offset,
            };
            runtime.begin_block();
            runtime
                .process_source_segment_with_volume(
                    offset as u64,
                    &mut actual[offset..offset + count],
                    &mut onsets,
                    &[],
                    &mut events,
                    None,
                    &hex_lattice::EMPTY_DEVICE_RESOURCES,
                    &mut volume,
                    Some(&values),
                )
                .unwrap();
            runtime.commit_block();
            offset += count;
        }
        assert_eq!(actual, expected);
        assert!(actual.iter().any(|frame| frame[0] != 0.0));
        let mut restored = [[0.0; 2]; 64];
        runtime.begin_block();
        runtime
            .process_source_segment(
                512,
                &mut restored,
                &mut onsets,
                &[],
                &mut events,
                None,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
                None,
                Some(&Base),
            )
            .unwrap();
        runtime.commit_block();
        assert_eq!(restored, base_expected);
    }
}

#[test]
fn refused_values_roll_back_prior_segments_and_oversized_segments_refuse_before_staging() {
    let factory = factory();
    let mut runtime = MutableInstrumentTrackRuntime::new(factory.as_ref(), 48000).unwrap();
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    let mut onsets = onsets();
    runtime.begin_block();
    let mut prefix = [[0.0; 2]; 128];
    runtime
        .process_source_segment(
            0,
            &mut prefix,
            &mut onsets,
            &[],
            &mut events,
            None,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
            None,
            Some(&Values),
        )
        .unwrap();
    let mut untouched = [[0.25; 2]; 16];
    assert!(
        runtime
            .process_source_segment(
                128,
                &mut untouched,
                &mut onsets,
                &[],
                &mut events,
                None,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
                None,
                Some(&InvalidValues),
            )
            .is_err()
    );
    assert_eq!(untouched, [[0.25; 2]; 16]);
    runtime.rollback_block();
    assert!(runtime.committed_is_quiescent());
    runtime.begin_block();
    let mut replay = [[0.0; 2]; 128];
    runtime
        .process_source_segment(
            0,
            &mut replay,
            &mut onsets,
            &[],
            &mut events,
            None,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
            None,
            Some(&Values),
        )
        .unwrap();
    assert_eq!(replay, prefix);
    runtime.rollback_block();
    runtime.begin_block();
    let mut oversized = vec![[0.25; 2]; hex_lattice::MAX_INSTRUMENT_AUTOMATION_FRAMES + 1];
    events.clear();
    assert!(
        runtime
            .process_source_segment(
                0,
                &mut oversized,
                &mut onsets,
                &[],
                &mut events,
                None,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
                None,
                Some(&Values),
            )
            .is_err()
    );
    assert!(events.is_empty());
    assert!(oversized.iter().all(|frame| *frame == [0.25; 2]));
    runtime.rollback_block();
    assert!(runtime.committed_is_quiescent());
}
