//! Prepared callback applies the loop-end envelope without allocation.
use super::*;
use hex_engine::{Audio, PreparedRevision, SineTable};
use hex_project::{Command, Created, Editor, Project};
use std::collections::BTreeMap;

#[test]
fn loop_end_fades_without_callback_allocations() {
    const RATE: u32 = 48_000;
    const START: usize = 41;
    const END: usize = 642;
    const FRAMES: usize = 2_048;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: RATE,
            frames: 1_024,
        })
        .unwrap()
    else {
        panic!("audio import result");
    };
    editor
        .execute(Command::SetLoopRegion {
            start: START as u64,
            end: END as u64,
            enabled: true,
        })
        .unwrap();
    let samples = vec![[0.5, -0.25]; 1_024];
    let media = BTreeMap::from([(asset, Audio::new(RATE, samples.clone()).unwrap())]);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    let (mut control, mut callback) = channel(RATE).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(revision).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; FRAMES];
    let allocations = allocation_counter::measure(|| {
        for block in output.chunks_mut(64) {
            callback.begin_block();
            callback.render(block).unwrap();
        }
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    for (frame, actual) in output.iter().enumerate() {
        let source = if frame < END {
            frame
        } else {
            START + (frame - END) % (END - START)
        };
        let remaining = END - source;
        let gain = if remaining <= 128 {
            (remaining - 1) as f32 / 127.0
        } else {
            1.0
        };
        let expected = [samples[source][0] * gain, samples[source][1] * gain];
        assert_eq!(*actual, expected, "frame {frame}");
    }
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn edited_loop_with_a_source_effect_follows_the_running_cursor_without_allocations() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "source effect".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("instrument track");
    };
    let source = editor.project().track(track).unwrap().source_id().unwrap();
    let parent = editor
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    editor
        .execute(Command::BindTrackSource {
            track,
            source: None,
        })
        .unwrap();
    let utility = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    editor
        .execute(Command::InsertNestedDevice {
            parent,
            state: utility,
        })
        .unwrap();
    editor
        .execute(Command::BindTrackSource {
            track,
            source: Some(source),
        })
        .unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "position witness".into(),
            path: "loop.wav".into(),
            sample_rate: 48_000,
            frames: 2_048,
        })
        .unwrap()
    else {
        panic!("audio import");
    };
    let samples: Vec<_> = (0..2048).map(|frame| [frame as f32 / 4096.0; 2]).collect();
    let media = BTreeMap::from([(asset, Audio::new(48_000, samples.clone()).unwrap())]);
    let table = Arc::new(SineTable::new());
    let mut prepare = |start, end, enabled| {
        editor
            .execute(Command::SetLoopRegion {
                start,
                end,
                enabled,
            })
            .unwrap();
        Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None).unwrap(),
        )
    };
    for section in [false, true] {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let revision = prepare(64, 512, true);
        let playback = if section {
            let region = revision.arrangement().loop_region().unwrap();
            let runtime = hex_engine::SessionPlayback::prepare_section_arrangement_preroll(
                revision,
                None,
                region,
                0,
                &AtomicBool::new(false),
            )
            .unwrap();
            let playback = Playback::from_interactive_arrangement_preroll(runtime).unwrap();
            assert_eq!(
                playback.section_loop(),
                None,
                "section uses the saved marker"
            );
            playback
        } else {
            Playback::try_from_prepared_with_session(revision).unwrap()
        };
        control.publish(playback).unwrap();
        if !section {
            control.command(PlaybackCommand::Play).unwrap();
        }
        for _ in 0..if section { 9 } else { 10 } {
            callback.begin_block();
            callback.render(&mut [[0.0; 2]; 64]).unwrap();
        }
        assert_eq!(control.status.position.load(Acquire), 192);

        // Behind, ahead, around the cursor, and disabled. Publish before entering
        // the measured scope, and reclaim the displaced graph on control.
        for (start, end, enabled, frames, expected) in [
            (16, 128, true, 64, 256),
            (384, 640, true, 416, 416),
            (400, 600, true, 200, 416),
            (400, 600, false, 256, 672),
        ] {
            let first = control.status.position.load(Acquire);
            let playback =
                Playback::try_from_prepared_with_session(prepare(start, end, enabled)).unwrap();
            let revision = control.publish(playback).unwrap();
            let mut output = [[0.0; 2]; 416];
            let measured = allocation_counter::measure(|| {
                for chunk in output[..frames].chunks_mut(37) {
                    callback.begin_block();
                    callback.render(chunk).unwrap();
                }
            });
            assert_eq!(control.status.applied_revision.load(Acquire), revision);
            assert_eq!(control.status.position.load(Acquire), expected);
            assert!(control.status.playing.load(Acquire));
            assert_eq!(output[0], samples[first as usize], "edit never seeks");
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            control.reclaim();
        }
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[test]
fn shared_source_utility_gain_and_mute_retarget_without_callback_allocations() {
    for (mute, settled, prefix) in [
        (false, false, None),
        (true, false, None),
        (true, true, None),
        (true, false, Some(11)),
        (true, true, Some(12)),
    ] {
        for partition in [1, 7, 64, 128, 257] {
            source_utility_matrix_callback_retarget(mute, settled, partition, prefix, None);
        }
    }
}

#[test]
fn shared_source_filtered_utility_mute_handoff_is_allocation_free() {
    for filter in [11, 12] {
        for settled in [false, true] {
            for partition in [1, 7, 64, 128, 257] {
                source_utility_matrix_callback_retarget(
                    true,
                    settled,
                    partition,
                    None,
                    Some(filter),
                );
            }
        }
    }
}

fn source_utility_matrix_callback_retarget(
    mute: bool,
    settled: bool,
    partition: usize,
    prefix: Option<u32>,
    filter: Option<u32>,
) {
    use hex_project::BeatTime;
    let target_gain = if mute { 0.0 } else { 2.0 };
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(receiver) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "receiver".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("receiver")
    };
    let source = editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let instrument = editor
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    if let Some(prefix) = prefix {
        let initial = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_engine::DeviceTypeId::from_static("hex.utility"),
        )
        .unwrap();
        let state = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Effect,
            &initial,
            hex_lattice_contract::ParameterId::from_static(prefix),
            hex_lattice_contract::ParameterValue::Bool(true),
        )
        .unwrap();
        editor
            .execute(Command::InsertNestedDevice {
                parent: instrument,
                state,
            })
            .unwrap();
    }
    let mut state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    if let Some(filter) = filter {
        state = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Effect,
            &state,
            hex_lattice_contract::ParameterId::from_static(filter),
            hex_lattice_contract::ParameterValue::Bool(true),
        )
        .unwrap();
    }
    let Created::Device(effect) = editor
        .execute(Command::InsertNestedDevice {
            parent: instrument,
            state,
        })
        .unwrap()
    else {
        panic!("source effect")
    };
    let Created::Track(contributor) = editor
        .execute(Command::AddMidiTrack {
            name: "contributor".into(),
        })
        .unwrap()
    else {
        panic!("contributor")
    };
    editor
        .execute(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        })
        .unwrap();
    for (track, pitch) in [(receiver, 60), (contributor, 67)] {
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
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    let table = Arc::new(SineTable::new());
    let prepare = |editor: &Editor| {
        Arc::new(
            PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
                .unwrap(),
        )
    };
    let original = prepare(&editor);
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (mut reference_control, mut reference) = channel(48_000).unwrap();
    for ctl in [&mut control, &mut reference_control] {
        ctl.publish(Playback::try_from_prepared_with_session(Arc::clone(&original)).unwrap())
            .unwrap();
    }
    let mut prefix = [[0.0; 2]; 64];
    callback.begin_block();
    callback.render(&mut prefix).unwrap();
    reference.begin_block();
    reference.render(&mut prefix).unwrap();
    let parameter = hex_lattice_contract::ParameterId::from_static(if mute { 8 } else { 1 });
    let state = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Effect,
        editor.project().device(effect).unwrap().state(),
        parameter,
        if mute {
            hex_lattice_contract::ParameterValue::Bool(true)
        } else {
            hex_lattice_contract::ParameterValue::Float(2.0)
        },
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: effect,
            state: state.clone(),
        })
        .unwrap();
    let stopped_revision = control
        .publish(Playback::try_from_prepared_with_session(prepare(&editor)).unwrap())
        .unwrap();
    let counts = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut prefix).unwrap();
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        stopped_revision
    );
    assert!(prefix.iter().all(|frame| *frame == [0.0; 2]));
    control.reclaim();
    assert!(editor.undo());
    let restored = control
        .publish(Playback::try_from_prepared_with_session(prepare(&editor)).unwrap())
        .unwrap();
    callback.begin_block();
    callback.render(&mut prefix).unwrap();
    assert_eq!(control.status.applied_revision.load(Acquire), restored);
    control.reclaim();
    for ctl in [&mut control, &mut reference_control] {
        ctl.command(PlaybackCommand::Play).unwrap();
    }
    callback.begin_block();
    callback.render(&mut prefix).unwrap();
    reference.begin_block();
    reference.render(&mut prefix).unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: effect,
            state,
        })
        .unwrap();
    let revision = control
        .publish(Playback::try_from_prepared_with_session(prepare(&editor)).unwrap())
        .unwrap();
    let mut actual = [[0.0; 2]; 128];
    let counts = allocation_counter::measure(|| {
        for chunk in actual.chunks_mut(partition) {
            callback.begin_block();
            callback.render(chunk).unwrap();
        }
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    let mut baseline = [[0.0; 2]; 128];
    for chunk in baseline.chunks_mut(partition) {
        reference.begin_block();
        reference.render(chunk).unwrap();
    }
    assert!(
        baseline.iter().flatten().any(|sample| sample.abs() > 1e-5),
        "source is silent"
    );
    for (i, (actual, old)) in actual.iter().zip(baseline).enumerate() {
        let factor = 1.0 + (target_gain - 1.0) * (i + 1) as f32 / 240.0;
        for channel in 0..2 {
            assert!(
                (actual[channel] - factor * old[channel]).abs() < 1e-4,
                "frame {i}"
            );
        }
    }
    if settled {
        let mut tail = [[0.0; 2]; 112];
        let mut baseline = tail;
        let counts = allocation_counter::measure(|| {
            for chunk in tail.chunks_mut(partition) {
                callback.begin_block();
                callback.render(chunk).unwrap();
            }
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        for chunk in baseline.chunks_mut(partition) {
            reference.begin_block();
            reference.render(chunk).unwrap();
        }
        for (i, (actual, old)) in tail.iter().zip(baseline).enumerate() {
            let factor = 1.0 - (128 + i + 1) as f32 / 240.0;
            for channel in 0..2 {
                assert!((actual[channel] - factor * old[channel]).abs() < 1e-4);
            }
        }
    }
    control.reclaim();
    assert!(editor.undo());
    let revision = control
        .publish(Playback::try_from_prepared_with_session(prepare(&editor)).unwrap())
        .unwrap();
    let counts = allocation_counter::measure(|| {
        for chunk in actual.chunks_mut(partition) {
            callback.begin_block();
            callback.render(chunk).unwrap();
        }
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    for chunk in baseline.chunks_mut(partition) {
        reference.begin_block();
        reference.render(chunk).unwrap();
    }
    let start = if settled {
        0.0
    } else {
        1.0 + (target_gain - 1.0) * 128.0 / 240.0
    };
    for (i, (actual, old)) in actual.iter().zip(baseline).enumerate() {
        let factor = start + (1.0 - start) * (i + 1) as f32 / 240.0;
        for channel in 0..2 {
            assert!(
                (actual[channel] - factor * old[channel]).abs() < 1e-4,
                "undo frame {i}"
            );
        }
    }
    control.reclaim();
    assert!(editor.redo());
    let pending = control
        .publish(Playback::try_from_prepared_with_session(prepare(&editor)).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Stop).unwrap();
    let counts = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 256]).unwrap();
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(!control.status.playing.load(Acquire));
    assert!(
        control.status.applied_revision.load(Acquire) == pending
            || control
                .status
                .rejected_revision()
                .is_some_and(|rejected| rejected.0 == pending),
        "pending publication must have a terminal when Stop wins"
    );
    for (ctl, cb) in [
        (&mut control, &mut callback),
        (&mut reference_control, &mut reference),
    ] {
        ctl.request_shutdown();
        cb.begin_block();
        ctl.reclaim();
    }
}

#[test]
fn shared_source_utility_matrix_handoff_preserves_pcm_without_allocations() {
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::BeatTime;
    for (dc, bass, equivalent, polarity, channel_choice) in [
        (true, false, false, None, None),
        (false, true, false, None, None),
        (false, false, true, None, None),
        (true, false, true, None, None),
        (false, true, true, None, None),
        (false, false, false, Some(5), None),
        (false, false, false, Some(6), None),
        (true, false, false, Some(5), None),
        (true, false, false, Some(6), None),
        (false, true, false, Some(5), None),
        (false, true, false, Some(6), None),
        (false, false, false, None, Some(1)),
        (true, false, false, None, Some(2)),
        (false, true, false, None, Some(3)),
    ] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(receiver) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "receiver".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("receiver")
        };
        let source = editor
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        let instrument = editor
            .project()
            .native_source(source)
            .unwrap()
            .instrument()
            .id();
        let mut state = hex_devices::utility::UTILITY_DEFINITION
            .default_state()
            .unwrap();
        if dc || bass {
            state = hex_devices::utility::UTILITY_DEFINITION
                .patch_parameter(
                    &state,
                    if dc {
                        hex_devices::utility::parameter::DC_REMOVE
                    } else {
                        hex_devices::utility::parameter::BASS_MONO
                    },
                    ParameterValue::Bool(true),
                )
                .unwrap();
        }
        let Created::Device(effect) = editor
            .execute(Command::InsertNestedDevice {
                parent: instrument,
                state,
            })
            .unwrap()
        else {
            panic!("effect")
        };
        let Created::Track(contributor) = editor
            .execute(Command::AddMidiTrack {
                name: "contributor".into(),
            })
            .unwrap()
        else {
            panic!("contributor")
        };
        editor
            .execute(Command::BindTrackSource {
                track: contributor,
                source: Some(source),
            })
            .unwrap();
        for (track, pitch) in [(receiver, 60), (contributor, 67)] {
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
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
        let table = Arc::new(SineTable::new());
        let prepare = |editor: &Editor| {
            Arc::new(
                PreparedRevision::prepare(
                    editor.project(),
                    &BTreeMap::new(),
                    Arc::clone(&table),
                    None,
                )
                .unwrap(),
            )
        };
        let original = prepare(&editor);
        let mut reference = hex_engine::SessionPlayback::new(Arc::clone(&original)).unwrap();
        reference
            .apply(hex_engine::SessionPlaybackCommand::Play)
            .unwrap();
        let (mut control, mut callback) = channel(48_000).unwrap();
        control
            .publish(Playback::try_from_prepared_with_session(original).unwrap())
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let mut actual = [[0.0; 2]; 64];
        let mut expected = [[0.0; 2]; 64];
        callback.begin_block();
        callback.render(&mut actual).unwrap();
        reference.render(&mut expected).unwrap();
        assert!(
            expected.iter().flatten().any(|x| x.abs() > 1e-5),
            "silent source"
        );
        let state = hex_devices::utility::UTILITY_DEFINITION
            .patch_parameter(
                editor.project().device(effect).unwrap().state(),
                if channel_choice.is_some() {
                    hex_devices::utility::parameter::CHANNEL
                } else if let Some(polarity) = polarity {
                    hex_lattice_contract::ParameterId::from_static(polarity)
                } else if equivalent {
                    hex_devices::utility::parameter::MID_SIDE_ENABLED
                } else {
                    hex_devices::utility::parameter::GAIN
                },
                if let Some(channel) = channel_choice {
                    ParameterValue::Choice(channel)
                } else if equivalent || polarity.is_some() {
                    ParameterValue::Bool(true)
                } else {
                    ParameterValue::Float(2.0)
                },
            )
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: effect,
                state,
            })
            .unwrap();
        for undo in [false, true] {
            if undo {
                assert!(editor.undo());
            }
            let next = prepare(&editor);
            let mut reference_next = hex_engine::SessionPlayback::new(Arc::clone(&next)).unwrap();
            reference_next
                .try_inherit_state_live_from(&mut reference)
                .unwrap();
            reference = reference_next;
            let revision = control
                .publish(Playback::try_from_prepared_with_session(next).unwrap())
                .unwrap();
            let counts = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(&mut actual).unwrap();
            });
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert_eq!(control.status.applied_revision.load(Acquire), revision);
            reference.render(&mut expected).unwrap();
            for (a, b) in actual.iter().zip(expected) {
                assert!(
                    (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5,
                    "dc={dc} bass={bass} equivalent={equivalent} undo={undo}: {a:?} vs {b:?}"
                );
            }
            control.reclaim();
        }
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[test]
fn first_shared_source_utility_attachment_preserves_pcm_without_callback_allocations() {
    use hex_lattice_contract::{ParameterId, ParameterValue};
    use hex_project::BeatTime;
    const LOCAL: [f32; 2] = [0.125, -0.25];
    for partition in [1, 7, 64, 128, 257] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(receiver) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "receiver".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("receiver")
        };
        let source = editor
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        let instrument = editor
            .project()
            .native_source(source)
            .unwrap()
            .instrument()
            .id();
        let Created::Track(contributor) = editor
            .execute(Command::AddMidiTrack {
                name: "contributor".into(),
            })
            .unwrap()
        else {
            panic!("contributor")
        };
        editor
            .execute(Command::BindTrackSource {
                track: contributor,
                source: Some(source),
            })
            .unwrap();
        for (track, pitch) in [(receiver, 60), (contributor, 67)] {
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
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "local.wav".into(),
                sample_rate: 48_000,
                frames: 512,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        editor
            .execute(Command::PlaceClip {
                track: receiver,
                asset,
                start: 0,
                duration: 512,
                source_offset: 0,
                source_length: 512,
                looping: false,
            })
            .unwrap();
        let media = BTreeMap::from([(asset, Audio::new(48_000, vec![LOCAL; 512]).unwrap())]);
        let table = Arc::new(SineTable::new());
        let prepare = |editor: &Editor| {
            Arc::new(
                PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None)
                    .unwrap(),
            )
        };
        let original = prepare(&editor);
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (mut reference_control, mut reference) = channel(48_000).unwrap();
        for ctl in [&mut control, &mut reference_control] {
            ctl.publish(Playback::try_from_prepared_with_session(Arc::clone(&original)).unwrap())
                .unwrap();
            ctl.command(PlaybackCommand::Play).unwrap();
        }
        for cb in [&mut callback, &mut reference] {
            cb.begin_block();
            cb.render(&mut [[0.0; 2]; 64]).unwrap();
        }
        let default = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_engine::DeviceTypeId::from_static("hex.utility"),
        )
        .unwrap();
        let state = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Effect,
            &default,
            ParameterId::from_static(1),
            ParameterValue::Float(2.0),
        )
        .unwrap();
        editor
            .execute(Command::InsertNestedDevice {
                parent: instrument,
                state,
            })
            .unwrap();
        let revision = control
            .publish(Playback::try_from_prepared_with_session(prepare(&editor)).unwrap())
            .unwrap();
        let mut actual = [[0.0; 2]; 128];
        let counts = allocation_counter::measure(|| {
            for chunk in actual.chunks_mut(partition) {
                callback.begin_block();
                callback.render(chunk).unwrap();
            }
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        let mut baseline = [[0.0; 2]; 128];
        for chunk in baseline.chunks_mut(partition) {
            reference.begin_block();
            reference.render(chunk).unwrap();
        }
        assert!(
            baseline
                .iter()
                .any(|frame| (frame[0] - LOCAL[0]).abs() > 1e-5)
        );
        for (i, (actual, dry)) in actual.iter().zip(baseline).enumerate() {
            for channel in 0..2 {
                let expected = 2.0 * (dry[channel] - LOCAL[channel]) + LOCAL[channel];
                assert!(
                    (actual[channel] - expected).abs() < 1e-4,
                    "partition={partition} frame={i} channel={channel} actual={} expected={expected}",
                    actual[channel]
                );
            }
        }
        control.reclaim();
        assert!(editor.undo());
        let removed = control
            .publish(Playback::try_from_prepared_with_session(prepare(&editor)).unwrap())
            .unwrap();
        let mut refused_audio = [[0.0; 2]; 64];
        let counts = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut refused_audio).unwrap();
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        assert_eq!(
            control
                .status
                .rejected_revision()
                .map(|rejected| rejected.0),
            Some(removed)
        );
        let mut dry_after = [[0.0; 2]; 64];
        reference.begin_block();
        reference.render(&mut dry_after).unwrap();
        for (actual, dry) in refused_audio.iter().zip(dry_after) {
            for channel in 0..2 {
                let expected = 2.0 * (dry[channel] - LOCAL[channel]) + LOCAL[channel];
                assert!((actual[channel] - expected).abs() < 1e-4);
            }
        }
        control.command(PlaybackCommand::Stop).unwrap();
        let counts = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut [[0.0; 2]; 256]).unwrap();
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(!control.status.playing.load(Acquire));
        control.reclaim();
        let removed_after_stop = control
            .publish(Playback::try_from_prepared_with_session(prepare(&editor)).unwrap())
            .unwrap();
        let counts = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut [[0.0; 2]; 1]).unwrap();
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            removed_after_stop
        );
        for (ctl, cb) in [
            (&mut control, &mut callback),
            (&mut reference_control, &mut reference),
        ] {
            ctl.request_shutdown();
            cb.begin_block();
            ctl.reclaim();
        }
    }
}

#[test]
fn appending_neutral_source_utility_preserves_audio_without_callback_allocations() {
    use hex_lattice_contract::{ParameterId, ParameterValue};
    use hex_project::BeatTime;
    const LOCAL: [f32; 2] = [0.125, -0.25];
    for partition in [1, 7, 64, 128, 257] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(receiver) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "receiver".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("receiver")
        };
        let source = editor
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        let instrument = editor
            .project()
            .native_source(source)
            .unwrap()
            .instrument()
            .id();
        let Created::Track(contributor) = editor
            .execute(Command::AddMidiTrack {
                name: "contributor".into(),
            })
            .unwrap()
        else {
            panic!("contributor")
        };
        editor
            .execute(Command::BindTrackSource {
                track: contributor,
                source: Some(source),
            })
            .unwrap();
        for (track, pitch) in [(receiver, 60), (contributor, 67)] {
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
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "local.wav".into(),
                sample_rate: 48_000,
                frames: 512,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        editor
            .execute(Command::PlaceClip {
                track: receiver,
                asset,
                start: 0,
                duration: 512,
                source_offset: 0,
                source_length: 512,
                looping: false,
            })
            .unwrap();
        let media = BTreeMap::from([(asset, Audio::new(48_000, vec![LOCAL; 512]).unwrap())]);
        let table = Arc::new(SineTable::new());
        let prepare = |editor: &Editor| {
            Arc::new(
                PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None)
                    .unwrap(),
            )
        };
        let first = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_engine::DeviceTypeId::from_static("hex.utility"),
        )
        .unwrap();
        let first = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Effect,
            &first,
            ParameterId::from_static(12),
            ParameterValue::Bool(true),
        )
        .unwrap();
        let first = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Effect,
            &first,
            ParameterId::from_static(2),
            ParameterValue::Float(1.0),
        )
        .unwrap();
        editor
            .execute(Command::InsertNestedDevice {
                parent: instrument,
                state: first,
            })
            .unwrap();
        let original = prepare(&editor);
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (mut reference_control, mut reference) = channel(48_000).unwrap();
        for ctl in [&mut control, &mut reference_control] {
            ctl.publish(Playback::try_from_prepared_with_session(Arc::clone(&original)).unwrap())
                .unwrap();
            ctl.command(PlaybackCommand::Play).unwrap();
        }
        for cb in [&mut callback, &mut reference] {
            cb.begin_block();
            cb.render(&mut [[0.0; 2]; 64]).unwrap();
        }
        let default = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_engine::DeviceTypeId::from_static("hex.utility"),
        )
        .unwrap();
        let state = default;
        editor
            .execute(Command::InsertNestedDevice {
                parent: instrument,
                state,
            })
            .unwrap();
        let revision = control
            .publish(Playback::try_from_prepared_with_session(prepare(&editor)).unwrap())
            .unwrap();
        let mut actual = [[0.0; 2]; 128];
        let counts = allocation_counter::measure(|| {
            for chunk in actual.chunks_mut(partition) {
                callback.begin_block();
                callback.render(chunk).unwrap();
            }
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        let mut baseline = [[0.0; 2]; 128];
        for chunk in baseline.chunks_mut(partition) {
            reference.begin_block();
            reference.render(chunk).unwrap();
        }
        assert!(
            baseline
                .iter()
                .any(|frame| (frame[0] - LOCAL[0]).abs() > 1e-5)
        );
        for (i, (actual, dry)) in actual.iter().zip(baseline).enumerate() {
            for channel in 0..2 {
                let expected = dry[channel];
                assert!(
                    (actual[channel] - expected).abs() < 1e-4,
                    "partition={partition} frame={i} channel={channel} actual={} expected={expected}",
                    actual[channel]
                );
            }
        }
        for (ctl, cb) in [
            (&mut control, &mut callback),
            (&mut reference_control, &mut reference),
        ] {
            ctl.request_shutdown();
            cb.begin_block();
            ctl.reclaim();
        }
    }
}

#[test]
fn removing_utility_from_shared_chain_waits_for_stop_without_callback_allocations() {
    use hex_lattice_contract::{ParameterId, ParameterValue};
    use hex_project::BeatTime;
    for (remove_first, append_third, reorder) in [
        (true, false, false),
        (false, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        for partition in [1, 7, 64, 128, 257] {
            let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
            let Created::Track(receiver) = editor
                .execute(Command::InsertTrackWithDevice {
                    name: "receiver".into(),
                    before_track: None,
                    slot: hex_project::TrackDeviceSlot::Instrument,
                    state: hex_project::default_instrument_state().unwrap(),
                })
                .unwrap()
            else {
                panic!("receiver")
            };
            let source = editor
                .project()
                .track(receiver)
                .unwrap()
                .source_id()
                .unwrap();
            let instrument = editor
                .project()
                .native_source(source)
                .unwrap()
                .instrument()
                .id();
            let Created::Track(contributor) = editor
                .execute(Command::AddMidiTrack {
                    name: "contributor".into(),
                })
                .unwrap()
            else {
                panic!("contributor")
            };
            editor
                .execute(Command::BindTrackSource {
                    track: contributor,
                    source: Some(source),
                })
                .unwrap();
            for (track, pitch) in [(receiver, 60), (contributor, 67)] {
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
                        pitch,
                        channel: 0,
                        velocity: 100,
                    })
                    .unwrap();
            }
            const LOCAL: [f32; 2] = [0.125, -0.25];
            let Created::Asset(asset) = editor
                .execute(Command::AddAsset {
                    path: "local.wav".into(),
                    sample_rate: 48_000,
                    frames: 512,
                })
                .unwrap()
            else {
                panic!("asset")
            };
            editor
                .execute(Command::PlaceClip {
                    track: receiver,
                    asset,
                    start: 0,
                    duration: 512,
                    source_offset: 0,
                    source_length: 512,
                    looping: false,
                })
                .unwrap();
            let default = hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Effect,
                hex_engine::DeviceTypeId::from_static("hex.utility"),
            )
            .unwrap();
            let filter = hex_engine::patch_native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                &default,
                ParameterId::from_static(12),
                ParameterValue::Bool(true),
            )
            .unwrap();
            let filter = hex_engine::patch_native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                &filter,
                ParameterId::from_static(2),
                ParameterValue::Float(1.0),
            )
            .unwrap();
            let Created::Device(first) = editor
                .execute(Command::InsertNestedDevice {
                    parent: instrument,
                    state: filter,
                })
                .unwrap()
            else {
                panic!("first")
            };
            let last_state = if reorder {
                hex_engine::patch_native_device_parameter_for_class(
                    hex_engine::NativeDeviceClass::Effect,
                    &default,
                    ParameterId::from_static(7),
                    ParameterValue::Bool(true),
                )
                .unwrap()
            } else {
                default.clone()
            };
            let Created::Device(last) = editor
                .execute(Command::InsertNestedDevice {
                    parent: instrument,
                    state: last_state,
                })
                .unwrap()
            else {
                panic!("last")
            };
            if append_third {
                editor
                    .execute(Command::InsertNestedDevice {
                        parent: instrument,
                        state: default,
                    })
                    .unwrap();
            }
            let media = BTreeMap::from([(asset, Audio::new(48_000, vec![LOCAL; 512]).unwrap())]);
            let table = Arc::new(SineTable::new());
            let prepare = |editor: &Editor| {
                Arc::new(
                    PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None)
                        .unwrap(),
                )
            };
            let original = prepare(&editor);
            let (mut control, mut callback) = channel(48_000).unwrap();
            let (mut reference_control, mut reference) = channel(48_000).unwrap();
            let mut active = 0;
            for ctl in [&mut control, &mut reference_control] {
                active = ctl
                    .publish(
                        Playback::try_from_prepared_with_session(Arc::clone(&original)).unwrap(),
                    )
                    .unwrap();
                ctl.command(PlaybackCommand::Play).unwrap();
            }
            for cb in [&mut callback, &mut reference] {
                cb.begin_block();
                cb.render(&mut [[0.0; 2]; 64]).unwrap();
            }
            let edit = if reorder {
                Command::MoveNativeSourceDevice {
                    source,
                    device: last,
                    before: Some(first),
                }
            } else {
                Command::RemoveDevice(if remove_first { first } else { last })
            };
            editor.execute(edit).unwrap();
            let removed = control
                .publish(Playback::try_from_prepared_with_session(prepare(&editor)).unwrap())
                .unwrap();
            let mut actual = [[0.0; 2]; 128];
            let counts = allocation_counter::measure(|| {
                for chunk in actual.chunks_mut(partition) {
                    callback.begin_block();
                    callback.render(chunk).unwrap();
                }
            });
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert_eq!(control.status.applied_revision.load(Acquire), active);
            assert_eq!(
                control
                    .status
                    .rejected_revision()
                    .map(|rejected| rejected.0),
                Some(removed)
            );
            let mut expected = [[0.0; 2]; 128];
            for chunk in expected.chunks_mut(partition) {
                reference.begin_block();
                reference.render(chunk).unwrap();
            }
            assert!(
                expected
                    .iter()
                    .any(|frame| (frame[0] - LOCAL[0]).abs() > 1e-5)
            );
            assert_eq!(
                actual, expected,
                "first={remove_first} partition={partition}"
            );
            control.command(PlaybackCommand::Stop).unwrap();
            let counts = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(&mut [[0.0; 2]; 256]).unwrap();
            });
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert!(!control.status.playing.load(Acquire));
            control.reclaim();
            let stopped = control
                .publish(Playback::try_from_prepared_with_session(prepare(&editor)).unwrap())
                .unwrap();
            let counts = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(&mut [[0.0; 2]; 1]).unwrap();
            });
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert_eq!(control.status.applied_revision.load(Acquire), stopped);
            for (ctl, cb) in [
                (&mut control, &mut callback),
                (&mut reference_control, &mut reference),
            ] {
                ctl.request_shutdown();
                cb.begin_block();
                ctl.reclaim();
            }
        }
    }
}

#[test]
fn stopped_live_input_history_mark_and_render_do_not_allocate() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(Command::AddTrack {
            name: "input".into(),
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = hex_engine::SessionPlayback::new(revision).unwrap();
    let mut output = [[0.0; 2]; 256];
    let mut result = Ok(());
    let counts = allocation_counter::measure(|| {
        playback.note_stopped_live_input();
        result = playback.render(&mut output);
    });
    result.unwrap();
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn live_track_audio_engine_render_has_no_allocations_or_frees() {
    for compensated in [false, true] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "input".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        if compensated {
            let limiter = hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Effect,
                hex_engine::DeviceTypeId::from_static("hex.limiter"),
            )
            .unwrap();
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: limiter,
                })
                .unwrap();
        }
        let revision = Arc::new(
            PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::new(SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut playback = hex_engine::SessionPlayback::new(revision).unwrap();
        playback
            .apply(hex_engine::SessionPlaybackCommand::Play)
            .unwrap();
        let mut output = [[0.0; 2]; 256];
        let input = [[0.25, -0.5]; 256];
        let mut result = Ok(());
        let counts = allocation_counter::measure(|| {
            result = playback.render_with_live_track_audio(
                &mut output,
                track,
                &input,
                None,
                None,
                None,
                &mut hex_lattice::RefuseForeign,
            );
        });
        result.unwrap();
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0),
            "compensated: {compensated}"
        );
    }
}

#[test]
fn live_track_audio_playback_wrapper_has_no_allocations_or_frees() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "input".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback =
        hex_engine::playback::Playback::try_from_prepared_with_session(revision).unwrap();
    playback.apply(hex_engine::PlaybackCommand::Play);
    let input = [[0.25, -0.5]; 512];
    let mut output = [[0.0; 2]; 512];
    let mut result = Ok(());
    let counts = allocation_counter::measure(|| {
        result = playback.render_with_live_track_audio(
            &mut output,
            track,
            &input,
            None,
            None,
            None,
            &mut hex_lattice::RefuseForeign,
        );
    });
    result.unwrap();
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(output.iter().any(|frame| *frame != [0.0; 2]));
}
