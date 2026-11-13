use super::*;
use hex_devices::sampler::{
    GateMode, Mode, PreparedSamplerPlan, Region, SampleChannels, SampleShape, Settings,
};
use hex_lattice::{
    BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetSlot};
use hex_project::{Command, Created, Editor, Project};

fn view(pcm: &[f32]) -> DeviceResourceView<'_> {
    DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: DeviceResourceIdentity {
            asset: DeviceAssetKey::new(1).unwrap(),
            content: [2; 32],
            derivation_version: 1,
            derivation: [3; 32],
        },
        sample_rate: 8000,
        pcm: DeviceResourcePcm::Mono(pcm),
    }
}
fn runtime(view: DeviceResourceView<'_>) -> MutableInstrumentTrackRuntime {
    let mut settings = Settings::new(Region {
        start: 0,
        end: view.pcm.frames(),
    });
    settings.mode = Mode::OneShot {
        gate: GateMode::Trigger,
    };
    let factory = PreparedSamplerPlan::new(
        SampleShape::new(view.pcm.frames(), 8000, SampleChannels::Mono).unwrap(),
        8000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(view)
    .unwrap()
    .prepare_runtime_on_control(4, 259, &hex_lattice::TuningTable::default())
    .unwrap();
    MutableInstrumentTrackRuntime::new(&factory, 8000).unwrap()
}
fn onsets() -> (TrackId, MutableOnsetBuffer) {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "gain".into(),
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
            velocity: 127,
        })
        .unwrap();
    let note = editor.project().midi_clip(clip).unwrap().notes()[0].id();
    let mut onsets = MutableOnsetBuffer::default();
    for (iteration, start) in [(0, 0), (1, 3)] {
        onsets
            .push(MutableNoteOnset {
                key: MutableOccurrenceKey::new(track, clip, note, iteration, 0),
                onset_beat: BeatTime::from_raw(start),
                gate_end_beat: BeatTime::ONE,
                source_start: start,
                source_gate_end: 8,
                pitch: 60,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
    }
    (track, onsets)
}
fn ramp(
    signature: MutableOccurrenceSignature,
    at: u64,
    _: usize,
    _: bool,
) -> Result<Option<VolumeSlice>, MutableInstrumentRuntimeError> {
    let ramp = if signature.key().iteration == 1 {
        VolumeRamp {
            start_frame: 0,
            end_frame: u64::MAX,
            start_gain: 0.5,
            end_gain: 0.5,
            next_boundary: None,
        }
    } else if at < 8 {
        VolumeRamp {
            start_frame: 0,
            end_frame: 8,
            start_gain: 0.0,
            end_gain: 1.0,
            next_boundary: Some(8),
        }
    } else {
        VolumeRamp {
            start_frame: 8,
            end_frame: u64::MAX,
            start_gain: 0.25,
            end_gain: 0.25,
            next_boundary: None,
        }
    };
    Ok(Some(VolumeSlice {
        at,
        ramp,
        terminal: None,
    }))
}
#[test]
fn real_sampler_gains_follow_exact_ids_onsets_jumps_and_arbitrary_partitions() {
    let pcm = [1.0; 16];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let (track, all_onsets) = onsets();
    let mut results = Vec::new();
    for partitions in [&[32][..], &[1, 3, 5, 2][..]] {
        let mut runtime = runtime(views[0]);
        let mut onsets = all_onsets.clone();
        let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
        let mut output = vec![[0.0; 2]; 32];
        let mut offset = 0;
        let mut index = 0;
        while offset < output.len() {
            let count = partitions[index % partitions.len()].min(output.len() - offset);
            runtime.begin_block();
            runtime
                .process_segment_with_volume(
                    track,
                    offset as u64,
                    offset as u64,
                    &mut output[offset..offset + count],
                    &mut onsets,
                    &[],
                    &mut events,
                    None,
                    &resources,
                    &mut ramp,
                    None,
                )
                .unwrap();
            runtime.commit_block();
            offset += count;
            index += 1;
        }
        for (i, frame) in output.iter().enumerate() {
            let first = if i < 8 {
                i as f32 / 8.0
            } else if i < 16 {
                0.25
            } else {
                0.0
            };
            let second = if (3..19).contains(&i) { 0.5 } else { 0.0 };
            assert_eq!(*frame, [first + second; 2], "frame {i}");
        }
        assert!(runtime.committed_is_quiescent());
        results.push(output);
    }
    assert_eq!(results[0], results[1]);
}
#[test]
fn outgoing_gain_holds_left_limit_and_retired_ids_are_never_dispatched() {
    let pcm = [1.0; 16];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let (track, mut onsets) = onsets();
    onsets.len = 1;
    let mut runtime = runtime(views[0]);
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    runtime.begin_block();
    let mut attack = [[0.0; 2]; 4];
    runtime
        .process_segment_with_volume(
            track,
            0,
            0,
            &mut attack,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            &mut ramp,
            None,
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(attack, [[0.0; 2], [0.125; 2], [0.25; 2], [0.375; 2]]);
    assert_eq!(f64::from_bits(runtime.primary[0].gain_at_end_bits), 0.5);
    runtime.begin_block();
    let mut output = [[0.0; 2]; 20];
    runtime
        .process_segment_with_resources(
            track,
            4,
            4,
            &mut output,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            None,
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(output[..12], [[0.5; 2]; 12]);
    assert_eq!(output[12..], [[0.0; 2]; 8]);
    assert!(runtime.committed_is_quiescent());
}
#[test]
fn foreign_live_id_refuses_before_output_and_rollback_keeps_original_history() {
    let pcm = [1.0; 16];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let (track, mut onsets) = onsets();
    onsets.len = 1;
    let mut runtime = runtime(views[0]);
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    runtime.begin_block();
    let mut first = [[0.0; 2]; 4];
    runtime
        .process_segment_with_volume(
            track,
            0,
            0,
            &mut first,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            &mut ramp,
            None,
        )
        .unwrap();
    runtime.commit_block();
    let id = runtime.primary[0].id;
    runtime.begin_block();
    runtime.scratch[0].id += 1;
    let mut rejected = [[9.0; 2]; 2];
    assert!(
        runtime
            .process_segment_with_resources(
                track,
                4,
                4,
                &mut rejected,
                &mut onsets,
                &[],
                &mut events,
                None,
                &resources,
                None,
            )
            .is_err()
    );
    assert_eq!(rejected, [[9.0; 2]; 2]);
    runtime.rollback_block();
    assert_eq!(runtime.primary[0].id, id);
    runtime.begin_block();
    let mut accepted = [[0.0; 2]; 2];
    runtime
        .process_segment_with_resources(
            track,
            4,
            4,
            &mut accepted,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            None,
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(accepted, [[0.5; 2]; 2]);
}

#[test]
fn retired_gain_never_reacquires_a_new_curve_or_a_rebased_old_clock() {
    let pcm = [1.0; 16];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let (track, mut onsets) = onsets();
    onsets.len = 1;
    let mut runtime = runtime(views[0]);
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    runtime.begin_block();
    let mut first = [[0.0; 2]; 4];
    runtime
        .process_segment_with_volume(
            track,
            0,
            0,
            &mut first,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            &mut ramp,
            None,
        )
        .unwrap();
    runtime.commit_block();
    runtime.begin_block();
    let mut retired = [[0.0; 2]; 1];
    runtime
        .process_segment_with_resources(
            track,
            4,
            4,
            &mut retired,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            None,
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(retired, [[0.5; 2]]);
    runtime.begin_block();
    let mut output = [[0.0; 2]; 2];
    let mut queries = 0;
    let mut incompatible_clock = |_, _, _, _| {
        queries += 1;
        Err(MutableInstrumentRuntimeError::InvalidSourceSpan)
    };
    runtime
        .process_segment_with_volume(
            track,
            5,
            5,
            &mut output,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            &mut incompatible_clock,
            None,
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(queries, 0);
    assert_eq!(output, [[0.5; 2]; 2]);
}

#[test]
fn finite_terminal_freezes_left_limit_before_a_new_clock_or_curve_is_visible() {
    let pcm = [1.0; 16];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let (track, mut onsets) = onsets();
    onsets.len = 1;
    let mut runtime = runtime(views[0]);
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    let mut finite = |signature, at, frames, retaining| {
        let mut slice = ramp(signature, at, frames, retaining)?.unwrap();
        slice.terminal = Some(8);
        Ok(Some(slice))
    };
    runtime.begin_block();
    let mut first = [[0.0; 2]; 8];
    runtime
        .process_segment_with_volume(
            track,
            0,
            0,
            &mut first,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            &mut finite,
            None,
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(first[7], [0.875; 2]);
    assert!(runtime.primary[0].gain_frozen);
    assert_eq!(f64::from_bits(runtime.primary[0].gain_at_end_bits), 1.0);
    runtime.begin_block();
    let mut output = [[0.0; 2]; 9];
    runtime
        .process_segment_with_volume(
            track,
            8,
            8,
            &mut output,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            &mut |_, _, _, _| panic!("finite tail queried a replacement clock"),
            None,
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(output[..8], [[1.0; 2]; 8]);
    assert_eq!(output[8], [0.0; 2]);
}
