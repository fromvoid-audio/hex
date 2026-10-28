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
    runtime_with_capacity(view, 4)
}
fn runtime_with_capacity(
    view: DeviceResourceView<'_>,
    voices: usize,
) -> MutableInstrumentTrackRuntime {
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
    .prepare_runtime_on_control(voices, 259, &hex_lattice::TuningTable::default())
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
    Ok(Some(VolumeSlice::single(at, ramp, None)))
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
fn product_slice_stops_at_earliest_factor_boundary_and_refuses_overflow() {
    let mut slice = VolumeSlice::single(
        2,
        VolumeRamp {
            start_frame: 0,
            end_frame: 12,
            start_gain: 0.0,
            end_gain: 1.0,
            next_boundary: Some(12),
        },
        None,
    );
    let extra = VolumeRamp {
        start_frame: 1,
        end_frame: 5,
        start_gain: 1.0,
        end_gain: 0.5,
        next_boundary: Some(5),
    };
    for _ in 1..hex_lattice::MAX_INSTRUMENT_GAIN_FACTORS {
        slice.push_factor(extra).unwrap();
    }
    assert_eq!(slice.remaining(8).unwrap(), 3);
    assert!(slice.gain(1, 4).is_err());
    assert!(slice.push_factor(extra).is_err());
    assert_eq!(slice.remaining(8).unwrap(), 3);
    assert!(slice.gain(1, 3).is_ok());
}

#[test]
fn factors_keep_independent_clock_positions_and_original_ramp_coordinates() {
    let outer = VolumeRamp {
        start_frame: 96,
        end_frame: 112,
        start_gain: 0.25,
        end_gain: 0.75,
        next_boundary: Some(112),
    };
    let old = VolumeRamp {
        start_frame: 4_990,
        end_frame: 5_010,
        start_gain: 0.5,
        end_gain: 1.5,
        next_boundary: Some(5_003),
    };
    let mut slice = VolumeSlice::single(100, outer, None);
    slice.push_factor_at(5_000, old).unwrap();
    assert_eq!(slice.remaining(8).unwrap(), 3);
    let (gain, _, _) = slice.gain(7, 3).unwrap();
    for offset in 0..3 {
        let expected = outer.gain_at(100 + offset).unwrap() * old.gain_at(5_000 + offset).unwrap();
        assert_eq!(gain.value_at_offset(offset), expected);
    }
    let mut invalid = VolumeSlice::single(100, outer, None);
    invalid.push_factor_at(5_003, old).unwrap();
    assert!(invalid.remaining(1).is_err());
}

#[test]
fn held_product_fits_seven_source_factors_beside_the_outer_factor() {
    let ramp = VolumeRamp {
        start_frame: 0,
        end_frame: u64::MAX,
        start_gain: 1.0,
        end_gain: 1.0,
        next_boundary: None,
    };
    let mut slice = VolumeSlice::single(0, ramp, None);
    let maximum = hex_lattice::MAX_INSTRUMENT_OCCURRENCE_GAIN.powi(7);
    slice.push_held_product(maximum).unwrap();
    let (gain, _, _) = slice.gain(1, 1).unwrap();
    assert_eq!(gain.value_at_offset(0), maximum);
    let mut invalid = VolumeSlice::single(0, ramp, None);
    assert!(invalid.push_held_product(maximum * 2.0).is_err());
    let mut invalid = VolumeSlice::single(0, ramp, None);
    assert!(invalid.push_held_product(f64::NAN).is_err());
}

#[test]
fn mutable_voice_multiplies_original_ramps_and_freezes_their_product() {
    let pcm = [1.0; 16];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let (track, all_onsets) = onsets();
    let mut outputs = Vec::new();
    for parts in [&[12][..], &[1, 3, 2][..]] {
        let mut runtime = runtime(views[0]);
        let mut onsets = all_onsets.clone();
        onsets.len = 1;
        let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
        let mut output = [[0.0; 2]; 12];
        let mut position = 0;
        let mut index = 0;
        while position < output.len() {
            let count = parts[index % parts.len()].min(output.len() - position);
            let mut lookup = |_, at, _, _| {
                if at >= 8 {
                    return Ok(None);
                }
                let mut slice = VolumeSlice::single(
                    at,
                    VolumeRamp {
                        start_frame: 0,
                        end_frame: 8,
                        start_gain: 0.0,
                        end_gain: 1.0,
                        next_boundary: Some(8),
                    },
                    Some(8),
                );
                slice.push_factor(VolumeRamp {
                    start_frame: 0,
                    end_frame: 8,
                    start_gain: 1.0,
                    end_gain: 0.5,
                    next_boundary: Some(8),
                })?;
                Ok(Some(slice))
            };
            runtime.begin_block();
            runtime
                .process_segment_with_volume(
                    track,
                    position as u64,
                    position as u64,
                    &mut output[position..position + count],
                    &mut onsets,
                    &[],
                    &mut events,
                    None,
                    &resources,
                    &mut lookup,
                )
                .unwrap();
            runtime.commit_block();
            position += count;
            index += 1;
        }
        for (frame, value) in output.iter().enumerate() {
            let t = (frame.min(8)) as f32 / 8.0;
            assert_eq!(*value, [t * (1.0 - 0.5 * t); 2]);
        }
        outputs.push(output);
    }
    assert_eq!(outputs[0], outputs[1]);
}

#[test]
fn one_voice_factor_can_hold_while_the_enclosing_clip_continues() {
    let pcm = [1.0; 16];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let (track, all_onsets) = onsets();
    let mut outputs = Vec::new();
    for parts in [&[12][..], &[1, 3, 2][..]] {
        let mut runtime = runtime(views[0]);
        let mut onsets = all_onsets.clone();
        onsets.len = 1;
        let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
        let mut output = [[0.0; 2]; 12];
        let mut position = 0;
        let mut index = 0;
        while position < output.len() {
            let count = parts[index % parts.len()].min(output.len() - position);
            let mut lookup = |_, at, _, _| {
                let outer = if at < 8 {
                    VolumeRamp {
                        start_frame: 0,
                        end_frame: 8,
                        start_gain: 1.0,
                        end_gain: 0.5,
                        next_boundary: Some(8),
                    }
                } else {
                    VolumeRamp {
                        start_frame: 8,
                        end_frame: u64::MAX,
                        start_gain: 0.5,
                        end_gain: 0.5,
                        next_boundary: None,
                    }
                };
                let inner = if at < 4 {
                    VolumeRamp {
                        start_frame: 0,
                        end_frame: 4,
                        start_gain: 0.0,
                        end_gain: 1.0,
                        next_boundary: Some(4),
                    }
                } else {
                    VolumeRamp {
                        start_frame: 4,
                        end_frame: u64::MAX,
                        start_gain: 1.0,
                        end_gain: 1.0,
                        next_boundary: None,
                    }
                };
                let mut slice = VolumeSlice::single(at, outer, None);
                slice.push_factor(inner)?;
                Ok(Some(slice))
            };
            runtime.begin_block();
            runtime
                .process_segment_with_volume(
                    track,
                    position as u64,
                    position as u64,
                    &mut output[position..position + count],
                    &mut onsets,
                    &[],
                    &mut events,
                    None,
                    &resources,
                    &mut lookup,
                )
                .unwrap();
            runtime.commit_block();
            position += count;
            index += 1;
        }
        for (frame, value) in output.iter().enumerate() {
            let outer = 1.0 - 0.5 * (frame.min(8) as f32 / 8.0);
            let inner = frame.min(4) as f32 / 4.0;
            assert_eq!(*value, [outer * inner; 2], "frame {frame}");
        }
        outputs.push(output);
    }
    assert_eq!(outputs[0], outputs[1]);
}

#[test]
fn handed_off_held_voice_factor_does_not_freeze_the_outer_curve() {
    let pcm = [1.0; 16];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let (track, mut onsets) = onsets();
    onsets.len = 1;
    let mut runtime = runtime(views[0]);
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    runtime.begin_block();
    runtime
        .process_segment_with_volume(
            track,
            0,
            0,
            &mut [[0.0; 2]; 1],
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            &mut ramp,
        )
        .unwrap();
    runtime.commit_block();
    runtime.primary[0].held_voice_gain_bits = Some(128.0_f64.to_bits());
    runtime.primary[0].gain_at_end_bits = 128.0_f64.to_bits();
    let mut outer = |_, at, _, held: bool| {
        assert!(held);
        Ok(Some(VolumeSlice::single(
            at,
            VolumeRamp {
                start_frame: 0,
                end_frame: 8,
                start_gain: 1.0,
                end_gain: 0.5,
                next_boundary: Some(8),
            },
            None,
        )))
    };
    let mut output = [[0.0; 2]; 7];
    runtime.begin_block();
    runtime
        .process_segment_with_volume(
            track,
            1,
            1,
            &mut output,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            &mut outer,
        )
        .unwrap();
    runtime.commit_block();
    for (index, sample) in output.iter().enumerate() {
        let expected = 128.0 * (1.0 - 0.5 * ((index + 1) as f32 / 8.0));
        assert_eq!(*sample, [expected; 2]);
    }
}

#[test]
fn arrangement_lookup_combines_clip_gain_with_the_note_owner_and_its_tail() {
    let (_, onsets) = onsets();
    let onset = onsets.spans[0].unwrap();
    let project = Project::new(8_000).unwrap();
    let timing = crate::PreparedTempoMap::prepare(project.tempo_map().clone(), 8_000).unwrap();
    let source = hex_project::MidiSourcePlayback::one_shot(BeatTime::ONE).unwrap();
    let curve = hex_project::MidiClipVolume::new(
        vec![
            hex_project::MidiClipVolumePoint::linear(
                BeatTime::ZERO,
                hex_project::MidiClipGain::new(0.5).unwrap(),
            ),
            hex_project::MidiClipVolumePoint::linear(
                BeatTime::ONE,
                hex_project::MidiClipGain::new(1.0).unwrap(),
            ),
        ],
        BeatTime::ONE,
    )
    .unwrap();
    let layer =
        hex_project::MidiVoiceGainLayer::new(BeatTime::ZERO, BeatTime::ONE, BeatTime::ONE, curve)
            .unwrap();
    let law = hex_project::MidiVoiceGain::new(vec![layer]).unwrap();
    let volumes = crate::midi_clip_volume::arrangement::PreparedArrangementVolumes::test_voice(
        onset.key.clip,
        onset.key.note,
        BeatTime::ZERO,
        source,
        &law,
        &timing,
    )
    .unwrap();
    for (frame, expected) in [(0, 0.125), (2_000, 0.1875), (4_000, 0.25), (4_010, 0.25)] {
        let outer = VolumeRamp {
            start_frame: 0,
            end_frame: u64::MAX,
            start_gain: 0.25,
            end_gain: 0.25,
            next_boundary: None,
        };
        let slice = crate::session_playback::render::append_arrangement_voice_gain(
            Some(&volumes),
            onset.key.clip,
            onset.key.note,
            onset.onset_beat,
            &timing,
            frame,
            1,
            VolumeSlice::single(frame, outer, None),
        )
        .unwrap();
        let (gain, _, _) = slice.gain(1, 1).unwrap();
        assert_eq!(gain.value_at_offset(0), expected, "frame {frame}");
    }
}

#[test]
fn session_lookup_keeps_old_and_new_loop_occurrence_gains_separate() {
    let (_, onsets) = onsets();
    let onset = onsets.spans[0].unwrap();
    let beat = |n| BeatTime::from_quarters(n).unwrap();
    let project = Project::new(8_000).unwrap();
    let timing = crate::PreparedTempoMap::prepare(project.tempo_map().clone(), 8_000).unwrap();
    let source =
        hex_project::MidiSourcePlayback::new(beat(4), beat(0), Some((beat(0), beat(4))), beat(8))
            .unwrap();
    let curve = hex_project::MidiClipVolume::new(
        vec![
            hex_project::MidiClipVolumePoint::linear(
                beat(0),
                hex_project::MidiClipGain::new(0.5).unwrap(),
            ),
            hex_project::MidiClipVolumePoint::linear(
                beat(4),
                hex_project::MidiClipGain::new(2.0).unwrap(),
            ),
        ],
        beat(4),
    )
    .unwrap();
    let layer = hex_project::MidiVoiceGainLayer::new(beat(0), beat(4), beat(4), curve).unwrap();
    let law = hex_project::MidiVoiceGain::new(vec![layer]).unwrap();
    let gains = crate::midi_clip_volume::voice::PreparedSessionVoiceGains::test_voice(
        onset.key.note,
        source,
        &law,
        &timing,
    )
    .unwrap();
    let identity = gains.preparation_identity().unwrap();
    assert!(identity.matches(&gains));
    let expected_bytes = gains.retained_bytes().unwrap() + identity.retained_bytes().unwrap();
    let mut ledger =
        crate::AudioRootStorageLedger::try_new_on_control(crate::AudioStorageLedgerReservation {
            data_rows: 32,
            handle_rows: 8,
            root_rows: 8,
            max_retained_bytes: usize::MAX,
            max_construction_peak_bytes: usize::MAX,
        })
        .unwrap();
    gains.visit_storage(&mut ledger).unwrap();
    identity.visit_storage(&mut ledger).unwrap();
    assert_eq!(ledger.summary().unwrap().data_bytes, expected_bytes);
    drop(ledger);
    let cue = crate::SessionMidiOrigin::new(beat(2), beat(4));
    assert_eq!(
        gains
            .voice(onset.key.note, beat(2), cue, &timing)
            .unwrap()
            .unwrap()
            .1,
        24_000
    );
    let shifted_source = source.with_playback_offset(beat(1)).unwrap();
    let shifted = crate::midi_clip_volume::voice::PreparedSessionVoiceGains::test_voice(
        onset.key.note,
        shifted_source,
        &law,
        &timing,
    )
    .unwrap();
    assert!(!identity.matches(&shifted));
    let shifted_cue = crate::SessionMidiOrigin::new(beat(2), beat(1));
    assert_eq!(
        shifted
            .voice(onset.key.note, beat(2), shifted_cue, &timing)
            .unwrap()
            .unwrap()
            .1,
        16_000
    );
    let outer = VolumeRamp {
        start_frame: 0,
        end_frame: u64::MAX,
        start_gain: 0.25,
        end_gain: 0.25,
        next_boundary: None,
    };
    let slice = crate::session_playback::render::append_session_voice_gain(
        Some(&shifted),
        onset.key.note,
        beat(2),
        shifted_cue,
        &timing,
        8_000,
        1,
        VolumeSlice::single(8_000, outer, None),
    )
    .unwrap();
    assert_eq!(slice.gain(1, 1).unwrap().0.value_at_offset(0), 0.3125);
    let origin = crate::SessionMidiOrigin::launch(beat(0));
    let outer = VolumeRamp {
        start_frame: 0,
        end_frame: u64::MAX,
        start_gain: 0.25,
        end_gain: 0.25,
        next_boundary: None,
    };
    for (onset_beat, expected) in [(beat(0), 0.5), (beat(4), 0.125)] {
        let slice = crate::session_playback::render::append_session_voice_gain(
            Some(&gains),
            onset.key.note,
            onset_beat,
            origin,
            &timing,
            16_000,
            1,
            VolumeSlice::single(16_000, outer, None),
        )
        .unwrap();
        let (gain, _, _) = slice.gain(1, 1).unwrap();
        assert_eq!(gain.value_at_offset(0), expected);
    }
}

#[test]
fn dense_sampler_applies_eight_factors_to_each_occurrence() {
    let pcm = [1.0; 16];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let (track, original) = onsets();
    let template = original.spans[0].unwrap();
    let mut onsets = MutableOnsetBuffer::default();
    for iteration in 0..32 {
        let mut onset = template;
        onset.key.iteration = iteration;
        onsets.push(onset).unwrap();
    }
    let mut runtime = runtime_with_capacity(views[0], 32);
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    let factor = VolumeRamp {
        start_frame: 0,
        end_frame: u64::MAX,
        start_gain: 0.5,
        end_gain: 0.5,
        next_boundary: None,
    };
    let mut lookup = |_, at, _, _| {
        let mut slice = VolumeSlice::single(at, factor, None);
        for _ in 1..hex_lattice::MAX_INSTRUMENT_GAIN_FACTORS {
            slice.push_factor(factor)?;
        }
        Ok(Some(slice))
    };
    let mut output = [[0.0; 2]; 8];
    runtime.begin_block();
    runtime
        .process_segment_with_volume(
            track,
            0,
            0,
            &mut output,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
            &mut lookup,
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(runtime.primary.len(), 32);
    assert_eq!(output, [[0.125; 2]; 8]);
}

#[test]
fn retired_voice_can_hold_a_product_above_one_factor_limit() {
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
        )
        .unwrap();
    runtime.commit_block();
    // Model a previously evaluated product of two valid factors at the
    // retirement cut. Its original source is no longer available to query.
    runtime.primary[0].gain_at_end_bits = 128.0_f64.to_bits();
    runtime.begin_block();
    let mut tail = [[0.0; 2]; 2];
    runtime
        .process_segment_with_resources(
            track,
            4,
            4,
            &mut tail,
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(tail, [[128.0; 2]; 2]);
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
                &resources
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
    let mut finite = |signature, at, frames, _| {
        let mut slice = ramp(signature, at, frames, false)?.unwrap();
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
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(output[..8], [[1.0; 2]; 8]);
    assert_eq!(output[8], [0.0; 2]);
}
