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
                contributor_fade_in: false,
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

#[test]
fn admitted_new_contributor_fades_in_without_changing_peer() {
    let pcm = [1.0; 128];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let (track, mut onsets) = onsets();
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 8).unwrap();
    editor
        .execute(Command::AddMidiTrack {
            name: "peer".into(),
        })
        .unwrap();
    let Created::Track(contributor) = editor
        .execute(Command::AddMidiTrack {
            name: "contributor".into(),
        })
        .unwrap()
    else {
        panic!("contributor")
    };
    assert_ne!(track, contributor);
    onsets.spans[0].as_mut().unwrap().source_gate_end = 64;
    let faded = onsets.spans[1].as_mut().unwrap();
    faded.key.track = contributor;
    faded.source_gate_end = 64;
    faded.source_start = 0;
    faded.contributor_fade_in = true;
    onsets.has_contributor_fade_in = true;
    let mut outputs = Vec::new();
    for partitions in [&[64][..], &[8][..], &[1, 3, 2][..]] {
        let mut runtime = runtime(views[0]);
        runtime.enable_source_live();
        let mut onsets = onsets.clone();
        let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
        let mut output = [[0.0; 2]; 64];
        let mut offset = 0;
        let mut index = 0;
        let mut half_gain = |_, at, _, _| {
            Ok(Some(VolumeSlice {
                at,
                ramp: VolumeRamp {
                    start_frame: 0,
                    end_frame: u64::MAX,
                    start_gain: 0.5,
                    end_gain: 0.5,
                    next_boundary: None,
                },
                terminal: None,
            }))
        };
        while offset < output.len() {
            let count = partitions[index % partitions.len()].min(output.len() - offset);
            runtime.begin_block();
            runtime
                .process_source_segment(
                    offset as u64,
                    &mut output[offset..offset + count],
                    &mut onsets,
                    &[],
                    &mut events,
                    None,
                    &resources,
                    Some(&mut half_gain),
                    None,
                )
                .unwrap();
            runtime.commit_block();
            offset += count;
            index += 1;
        }
        for (i, frame) in output.iter().enumerate() {
            let expected = 0.5 + 0.5 * (i.min(40) as f32 / 40.0);
            assert!(
                (frame[0] - expected).abs() < 0.000_001,
                "frame {i}: {frame:?}"
            );
            assert_eq!(frame[0], frame[1]);
        }
        outputs.push(output);
    }
    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0], outputs[2]);
}

#[test]
fn faded_onset_refuses_without_source_age_or_at_mid_segment() {
    for (source_age, rate, onset_start) in [
        (None, Some(8_000), 0),
        (Some(0), None, 0),
        (Some(0), Some(7_999), 0),
        (Some(0), Some(8_000), 1),
    ] {
        let (track, mut onsets) = onsets();
        onsets.len = 1;
        let onset = onsets.spans[0].as_mut().unwrap();
        onset.source_start = onset_start;
        onset.contributor_fade_in = true;
        onsets.has_contributor_fade_in = true;
        let mut scratch = Vec::with_capacity(MAX_MUTABLE_OCCURRENCE_PROOFS);
        let mut next_id = 0;
        let mut emitted = 0;
        let result = authored_gate::stage_authored_gates(
            &mut scratch,
            &mut next_id,
            &mut onsets,
            authored_gate::AuthoredGateContext {
                track: Some(track),
                performance_start: 0,
                source_start: 0,
                width: 4,
                source_age,
                contributor_fade_rate: rate,
                tail_frames: 0,
                max_active: 4,
                steals_releasing: false,
                live: 0,
                suppressions: &[],
            },
            |_| {
                emitted += 1;
                Ok(())
            },
        );
        assert!(matches!(
            result,
            Err(MutableInstrumentRuntimeError::InvalidSourceSpan)
        ));
        assert_eq!(emitted, 0);
        assert!(scratch.is_empty());
        assert_eq!(next_id, 0);
    }
}

#[test]
fn faded_onset_rollback_keeps_its_source_age_and_identity() {
    let pcm = [1.0; 16];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let (track, mut onsets) = onsets();
    onsets.len = 1;
    onsets.spans[0].as_mut().unwrap().contributor_fade_in = true;
    onsets.has_contributor_fade_in = true;
    let mut runtime = runtime(views[0]);
    runtime.enable_source_live();
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    let mut first = [[0.0; 2]; 4];
    runtime.begin_block();
    runtime
        .process_segment_with_resources(
            track,
            0,
            0,
            &mut first,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            None,
        )
        .unwrap();
    assert!(runtime.committed_ledger_is_empty());
    let mut retry = [[0.0; 2]; 4];
    runtime.begin_block();
    runtime
        .process_segment_with_resources(
            track,
            0,
            0,
            &mut retry,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            None,
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(first, retry);
    assert_eq!(runtime.occurrence_proofs().len(), 1);
    for (i, frame) in retry.iter().enumerate() {
        assert!(
            (frame[0] - i as f32 / 40.0).abs() < 0.000_001,
            "frame {i}: {frame:?}"
        );
    }
}

#[test]
fn contributor_fade_length_tracks_supported_sample_rates() {
    for (rate, frames) in [(8_000, 40), (44_100, 221), (96_000, 480), (192_000, 960)] {
        assert_eq!(ContributorFadeIn::new(rate).unwrap().frames(), frames);
    }
    assert!(ContributorFadeIn::new(7_999).is_none());
    assert!(ContributorFadeIn::new(192_001).is_none());
}

#[test]
fn admitted_live_onset_fades_without_attenuating_a_sounding_peer() {
    for partition in [1, 7, 64] {
        let pcm = [1.0; 256];
        let views = [view(&pcm)];
        let resources = BorrowedDeviceResources(&views);
        let (track, mut onsets) = onsets();
        onsets.len = 1;
        onsets.spans[0].as_mut().unwrap().source_gate_end = 128;
        let input = crate::mutable_instrument::LiveInputId {
            track,
            generation: 1,
        };
        let mut runtime = runtime(views[0]);
        runtime.enable_source_live();
        let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
        runtime.begin_block();
        runtime
            .process_source_segment(
                0,
                &mut [[0.0; 2]; 7],
                &mut onsets,
                &[],
                &mut events,
                None,
                &resources,
                None,
                None,
            )
            .unwrap();
        runtime.commit_block();
        onsets.clear();
        runtime.begin_block();
        runtime
            .stage_live_events(
                input,
                &[NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 60,
                    velocity: 127,
                }],
                Some(1),
            )
            .unwrap();
        let mut output = [[0.0; 2]; 64];
        let mut start = 7;
        for chunk in output.chunks_mut(partition) {
            runtime
                .process_source_segment(
                    start,
                    chunk,
                    &mut onsets,
                    &[],
                    &mut events,
                    None,
                    &resources,
                    None,
                    None,
                )
                .unwrap();
            start += chunk.len() as u64;
        }
        runtime.commit_block();
        for (i, frame) in output.iter().enumerate() {
            let expected = 1.0 + i.min(40) as f32 / 40.0;
            assert!(
                (frame[0] - expected).abs() < 0.000_002,
                "frame {i}: {frame:?} expected {expected}"
            );
        }
        runtime.begin_block();
        runtime
            .stage_live_events(
                input,
                &[NativeInstrumentEvent::NoteOff {
                    frame_offset: 0,
                    occurrence_id: 1,
                }],
                None,
            )
            .unwrap();
        let voice = runtime
            .live
            .as_ref()
            .unwrap()
            .scratch
            .iter()
            .flatten()
            .next()
            .unwrap();
        assert!(
            voice.quiet_age.is_some(),
            "key-up must reach its source-owned voice"
        );
    }
}

#[test]
fn marked_live_onset_refuses_bad_offset_and_rolls_back_cleanly() {
    let pcm = [1.0; 128];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let (track, mut onsets) = onsets();
    onsets.clear();
    let input = crate::mutable_instrument::LiveInputId {
        track,
        generation: 1,
    };
    let mut runtime = runtime(views[0]);
    runtime.enable_source_live();
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    let note = NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 7,
        pitch: 60,
        velocity: 127,
    };
    runtime.begin_block();
    let late = NativeInstrumentEvent::NoteOn {
        frame_offset: 1,
        occurrence_id: 7,
        pitch: 60,
        velocity: 127,
    };
    assert!(matches!(
        runtime.stage_live_events(input, &[late], Some(7)),
        Err(MutableInstrumentRuntimeError::InvalidSourceSpan)
    ));
    assert!(matches!(
        runtime.stage_live_events(input, &[note, note], Some(7)),
        Err(MutableInstrumentRuntimeError::InvalidSourceSpan)
    ));
    let off = NativeInstrumentEvent::NoteOff {
        frame_offset: 0,
        occurrence_id: 7,
    };
    assert!(matches!(
        runtime.stage_live_events(input, &[note, off], Some(7)),
        Err(MutableInstrumentRuntimeError::InvalidSourceSpan)
    ));
    assert!(
        runtime
            .live
            .as_ref()
            .unwrap()
            .scratch
            .iter()
            .all(Option::is_none)
    );
    runtime.stage_live_events(input, &[note], Some(7)).unwrap();
    let mut first = [[0.0; 2]; 64];
    runtime
        .process_source_segment(
            0,
            &mut first,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            None,
            None,
        )
        .unwrap();
    runtime.rollback_block();
    runtime.begin_block();
    runtime.stage_live_events(input, &[note], Some(7)).unwrap();
    let mut retry = [[0.0; 2]; 64];
    runtime
        .process_source_segment(
            0,
            &mut retry,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            None,
            None,
        )
        .unwrap();
    assert_eq!(first, retry);
}

#[test]
fn marked_live_onset_refuses_a_provider_without_occurrence_gain() {
    let (track, _) = onsets();
    let input = crate::mutable_instrument::LiveInputId {
        track,
        generation: 1,
    };
    let mut runtime = MutableInstrumentTrackRuntime::new(
        &crate::computer_keyboard::clock_admission_tests::ClockRequiredInstrument,
        8000,
    )
    .unwrap();
    runtime.enable_source_live();
    runtime.begin_block();
    let note = NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 60,
        velocity: 127,
    };
    assert!(matches!(
        runtime.stage_live_events(input, &[note], Some(1)),
        Err(MutableInstrumentRuntimeError::InvalidSourceSpan)
    ));
    assert!(
        runtime
            .live
            .as_ref()
            .unwrap()
            .scratch
            .iter()
            .all(Option::is_none)
    );
}

#[test]
fn faded_onset_without_volume_crosses_the_fade_end_in_one_segment() {
    let pcm = [1.0; 128];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let (track, mut onsets) = onsets();
    onsets.len = 1;
    let onset = onsets.spans[0].as_mut().unwrap();
    onset.source_gate_end = 64;
    onset.contributor_fade_in = true;
    onsets.has_contributor_fade_in = true;
    let mut runtime = runtime(views[0]);
    runtime.enable_source_live();
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    let mut output = [[0.0; 2]; 64];
    runtime.begin_block();
    runtime
        .process_segment_with_resources(
            track,
            0,
            0,
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
    for (i, frame) in output.iter().enumerate() {
        let expected = i.min(40) as f32 / 40.0;
        assert!(
            (frame[0] - expected).abs() < 0.000_001,
            "frame {i}: {frame:?}"
        );
        assert_eq!(frame[0], frame[1]);
    }
}

#[test]
fn marked_mid_segment_onset_refuses_before_render_and_clear_resets_marker() {
    let pcm = [1.0; 16];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let (track, mut onsets) = onsets();
    let mut onset = onsets.spans[0].unwrap();
    onset.source_start = 1;
    onset.contributor_fade_in = true;
    onsets.clear();
    onsets.push(onset).unwrap();
    let mut runtime = runtime(views[0]);
    runtime.enable_source_live();
    let mut output = [[0.25; 2]; 8];
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    runtime.begin_block();
    let result = runtime.process_segment_with_resources(
        track,
        0,
        0,
        &mut output,
        &mut onsets,
        &[],
        &mut events,
        None,
        &resources,
        None,
    );
    assert!(matches!(
        result,
        Err(MutableInstrumentRuntimeError::InvalidSourceSpan)
    ));
    assert_eq!(output, [[0.25; 2]; 8]);
    assert!(runtime.committed_ledger_is_empty());
    onsets.clear();
    assert!(!onsets.has_contributor_fade_in);
    onset.contributor_fade_in = false;
    onsets.push(onset).unwrap();
    assert!(!onsets.has_contributor_fade_in);
}
