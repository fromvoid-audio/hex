//! Playable routing is compared with independent local-source PCM and transport owners.
use super::*;
use crate::PreparedSessionInventory;
use hex_project::{Command, Created, Editor, Project};
use std::collections::BTreeMap;

fn beat(frame: u64) -> BeatTime {
    BeatTime::from_raw((frame << 32) / 24_000)
}
struct Fixture {
    editor: Editor,
    tracks: [TrackId; 3],
    clips: [ClipId; 3],
    notes: [hex_project::NoteId; 3],
    media: BTreeMap<hex_project::AssetId, crate::Audio>,
}
fn fixture(routed: bool) -> Fixture {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let mut tracks = Vec::new();
    for _ in 0..3 {
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "bus".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap()
        else {
            panic!("track")
        };
        tracks.push(track);
    }
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let mut clips = Vec::new();
    let mut notes = Vec::new();
    for (index, &track) in tracks.iter().enumerate() {
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "notes".into(),
                source_length: beat(512),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        clips.push(clip);
        let pitch = if routed && index > 0 {
            84 + index as u8
        } else {
            60
        };
        for start in [0, 64] {
            let Created::Note(note) = editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: beat(start),
                    duration: beat(64),
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap()
            else {
                panic!("note")
            };
            if start == 0 {
                notes.push(note);
            }
        }
        let Created::Clip(arrangement) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: beat(512),
            })
            .unwrap()
        else {
            panic!("arrangement")
        };
        for start in [0, 64] {
            editor
                .execute(Command::AddMidiNote {
                    clip: arrangement,
                    start: beat(start),
                    duration: beat(64),
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
    }
    // Two independent destination algorithms make incorrect source-local instrument reuse audible.
    let device = editor
        .project()
        .track(tracks[2])
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: crate::default_native_device_state_for_class(
                crate::NativeDeviceClass::Instrument,
                hex_devices::fm4::device::FM4_TYPE_ID,
            )
            .unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::RemoveInstrument { track: tracks[0] })
        .unwrap();
    if routed {
        editor
            .execute(Command::SetMidiSource {
                destination: tracks[1],
                source: Some(tracks[0]),
            })
            .unwrap();
        editor
            .execute(Command::SetMidiSource {
                destination: tracks[2],
                source: Some(tracks[1]),
            })
            .unwrap();
    }
    Fixture {
        editor,
        tracks: [tracks[0], tracks[1], tracks[2]],
        clips: [clips[0], clips[1], clips[2]],
        notes: [notes[0], notes[1], notes[2]],
        media: BTreeMap::new(),
    }
}
fn prepare(f: &Fixture, previous: Option<&PreparedSessionInventory>) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(hex_devices::SineTable::new()),
            previous,
        )
        .unwrap(),
    )
}
fn start(revision: Arc<PreparedRevision>, clips: &[ClipId]) -> SessionPlayback {
    let mut runtime = SessionPlayback::new(revision).unwrap();
    for &clip in clips {
        runtime
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
    }
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    runtime
}
fn render(runtime: &mut SessionPlayback, frames: usize, partition: usize) -> Vec<Frame> {
    let mut pcm = vec![[0.0; 2]; frames];
    for chunk in pcm.chunks_mut(partition) {
        runtime.render(chunk).unwrap();
    }
    pcm
}

fn assert_pcm(actual: &[Frame], expected: &[Frame], tolerance: f32) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for channel in 0..2 {
            assert!(
                (actual[channel] - expected[channel]).abs() <= tolerance,
                "pcm differs at frame {index}, channel {channel}: actual={}, expected={}, tolerance={tolerance}",
                actual[channel],
                expected[channel]
            );
        }
    }
}

#[test]
fn routed_midi_arrangement_session_fanout_stop_relaunch_match_local_pcm_across_partitions() {
    let routed = fixture(true);
    let local = fixture(false);
    let revision = prepare(&routed, None);
    assert!(
        revision
            .arrangement()
            .signal_graph
            .as_ref()
            .is_some_and(|graph| !graph.needs_runtime()),
        "logical MIDI routing must retain the zero-latency block renderer"
    );
    let reference = prepare(&local, None);
    for partition in [1, 63, 256, 513] {
        for session in [false, true] {
            let mut actual = start(
                Arc::clone(&revision),
                if session { &routed.clips[..1] } else { &[] },
            );
            let mut expected = start(
                Arc::clone(&reference),
                if session { &local.clips[1..] } else { &[] },
            );
            let a = render(&mut actual, 320, partition);
            let b = render(&mut expected, 320, partition);
            assert_pcm(&a, &b, 1e-7);
            let mut exact = start(
                Arc::clone(&revision),
                if session { &routed.clips[..1] } else { &[] },
            );
            assert_pcm(&a, &render(&mut exact, 320, 1), 0.0);
            assert!(a.iter().any(|frame| frame[0] != 0.0));
            if session {
                actual
                    .apply(SessionPlaybackCommand::StopTrack {
                        track: routed.tracks[0],
                        quantization: LaunchQuantization::Immediate,
                    })
                    .unwrap();
                for track in &local.tracks[1..] {
                    expected
                        .apply(SessionPlaybackCommand::StopTrack {
                            track: *track,
                            quantization: LaunchQuantization::Immediate,
                        })
                        .unwrap();
                }
                assert_pcm(
                    &render(&mut actual, 320, partition),
                    &render(&mut expected, 320, partition),
                    1e-7,
                );
                for &clip in &routed.clips[..1] {
                    actual
                        .apply(SessionPlaybackCommand::LaunchClip {
                            clip,
                            quantization: LaunchQuantization::Immediate,
                        })
                        .unwrap();
                }
                for &clip in &local.clips[1..] {
                    expected
                        .apply(SessionPlaybackCommand::LaunchClip {
                            clip,
                            quantization: LaunchQuantization::Immediate,
                        })
                        .unwrap();
                }
                assert_pcm(
                    &render(&mut actual, 320, partition),
                    &render(&mut expected, 320, partition),
                    1e-7,
                );
            }
            actual.apply(SessionPlaybackCommand::Stop).unwrap();
            assert!(
                render(&mut actual, 16, partition)
                    .iter()
                    .all(|frame| *frame == [0.0; 2])
            );
        }
    }
}

#[test]
fn routed_midi_keeps_destination_audio_owner_independent() {
    let mut f = fixture(true);
    let Created::Scene(scene) = f
        .editor
        .execute(Command::AddScene {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::Asset(asset) = f
        .editor
        .execute(Command::AddAsset {
            path: "audio.wav".into(),
            sample_rate: 48_000,
            frames: 1024,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::SessionClip(clip) = f
        .editor
        .execute(Command::CreateSessionAudioClip {
            track: f.tracks[1],
            scene,
            name: "audio".into(),
            asset,
            source_start: 0,
            source_end: 1024,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    f.media.insert(
        asset,
        crate::Audio::new(48_000, vec![[0.03125, -0.015625]; 1024]).unwrap(),
    );
    let revision = prepare(&f, None);
    let mut actual = start(Arc::clone(&revision), &f.clips[..1]);
    let mut reference = start(revision, &f.clips[..1]);
    assert_pcm(
        &render(&mut actual, 63, 63),
        &render(&mut reference, 63, 63),
        0.0,
    );
    actual
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let expected = render(&mut reference, 320, 63);
    let pcm = render(&mut actual, 320, 63);
    for (frame, expected) in pcm.iter().zip(expected) {
        assert!((frame[0] - (expected[0] + 0.03125)).abs() < 1e-6);
        assert!((frame[1] - (expected[1] - 0.015625)).abs() < 1e-6);
    }
    assert_eq!(
        actual.owner(f.tracks[1]),
        Some(SessionPlaybackOwner::Session { clip })
    );
}

#[test]
fn routed_midi_layout_edit_refuses_before_mutation_and_gain_edit_carries_effective_owner() {
    let mut f = fixture(true);
    let revision = prepare(&f, None);
    let mut active = start(Arc::clone(&revision), &f.clips[..1]);
    let mut reference = start(Arc::clone(&revision), &f.clips[..1]);
    assert_pcm(
        &render(&mut active, 63, 63),
        &render(&mut reference, 63, 63),
        0.0,
    );
    // Rewiring a chain to the same resolved root still changes the authored MIDI layout.
    f.editor
        .execute(Command::SetMidiSource {
            destination: f.tracks[2],
            source: Some(f.tracks[0]),
        })
        .unwrap();
    let mut changed = SessionPlayback::new(prepare(&f, Some(revision.session()))).unwrap();
    assert!(changed.try_inherit_state_from(&mut active).is_err());
    assert_eq!(active.performance_frame(), 63);
    assert_pcm(
        &render(&mut active, 64, 63),
        &render(&mut reference, 64, 63),
        0.0,
    );
    assert!(f.editor.undo());
    f.editor
        .execute(Command::Gain {
            track: f.tracks[1],
            gain: 0.5,
        })
        .unwrap();
    let mut candidate = SessionPlayback::new(prepare(&f, Some(revision.session()))).unwrap();
    candidate.try_inherit_state_from(&mut active).unwrap();
    let mut first = fixture(false);
    first
        .editor
        .execute(Command::Gain {
            track: first.tracks[2],
            gain: 0.0,
        })
        .unwrap();
    let mut second = fixture(false);
    second
        .editor
        .execute(Command::Gain {
            track: second.tracks[1],
            gain: 0.0,
        })
        .unwrap();
    let mut first = start(prepare(&first, None), &first.clips[1..]);
    let mut second = start(prepare(&second, None), &second.clips[1..]);
    render(&mut first, 127, 63);
    render(&mut second, 127, 63);
    let first = render(&mut first, 320, 63);
    let second = render(&mut second, 320, 63);
    let expected: Vec<_> = first
        .iter()
        .zip(second)
        .enumerate()
        .map(|(index, (a, b))| {
            let gain = 1.0 - 0.5 * (index + 1).min(240) as f32 / 240.0;
            [a[0] * gain + b[0], a[1] * gain + b[1]]
        })
        .collect();
    assert_pcm(&render(&mut candidate, 320, 63), &expected, 1e-7);
    active.apply(SessionPlaybackCommand::Stop).unwrap();
    changed.try_inherit_state_from(&mut active).unwrap();
}

#[test]
fn routed_midi_source_edit_reconciles_each_destination_voice_against_local_source_oracle() {
    let mut f = fixture(true);
    let mut local = fixture(false);
    let revision = prepare(&f, None);
    let reference = prepare(&local, None);
    let mut old = start(Arc::clone(&revision), &f.clips[..1]);
    let mut oracle = start(Arc::clone(&reference), &local.clips[1..]);
    assert_pcm(&render(&mut old, 32, 1), &render(&mut oracle, 32, 1), 1e-7);
    for (fixture, indices) in [(&mut f, &[][..]), (&mut local, &[1usize, 2][..])] {
        let indices = if indices.is_empty() {
            &[0usize][..]
        } else {
            indices
        };
        for &index in indices {
            fixture
                .editor
                .execute(Command::UpdateMidiNote {
                    note: fixture.notes[index],
                    start: BeatTime::ZERO,
                    duration: beat(48),
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
    }
    let mut next = SessionPlayback::new(prepare(&f, Some(revision.session()))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut expected = SessionPlayback::new(prepare(&local, Some(reference.session()))).unwrap();
    expected.try_inherit_state_from(&mut oracle).unwrap();
    assert_pcm(
        &render(&mut next, 1024, 63),
        &render(&mut expected, 1024, 256),
        1e-7,
    );
}

#[test]
fn routed_session_operator_import_and_feedback_refusal_match_local_root_lifecycle() {
    for (parameter, value, supports_import) in [
        (
            hex_devices::operator_state::parameter::feedback(1),
            hex_lattice_contract::ParameterValue::Float(0.4),
            false,
        ),
        (
            hex_lattice_contract::ParameterId::from_static(20_001),
            hex_lattice_contract::ParameterValue::Bool(true),
            true,
        ),
    ] {
        let mut f = fixture(true);
        let mut local = fixture(false);
        let operator = crate::operator_legacy_test::legacy_operator_state();
        for fixture in [&mut f, &mut local] {
            let device = fixture
                .editor
                .project()
                .track(fixture.tracks[1])
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            fixture
                .editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: operator.clone(),
                })
                .unwrap();
        }
        let revision = prepare(&f, None);
        let reference = prepare(&local, None);
        let mut old = start(Arc::clone(&revision), &f.clips[..1]);
        let mut oracle = start(Arc::clone(&reference), &local.clips[1..]);
        assert_pcm(&render(&mut old, 32, 1), &render(&mut oracle, 32, 1), 1e-7);
        let state = crate::patch_native_device_parameter_for_class(
            crate::NativeDeviceClass::Instrument,
            &operator,
            parameter,
            value,
        )
        .unwrap();
        for fixture in [&mut f, &mut local] {
            let device = fixture
                .editor
                .project()
                .track(fixture.tracks[1])
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            fixture
                .editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: state.clone(),
                })
                .unwrap();
        }
        let mut next = SessionPlayback::new(prepare(&f, Some(revision.session()))).unwrap();
        let mut expected =
            SessionPlayback::new(prepare(&local, Some(reference.session()))).unwrap();
        if !supports_import {
            assert_eq!(
                expected.try_inherit_state_from(&mut oracle),
                Err(SessionPlaybackError::MutableInstrumentChase)
            );
            assert_eq!(
                next.try_inherit_state_from(&mut old),
                Err(SessionPlaybackError::MutableInstrumentChase)
            );
            assert_eq!(old.performance_frame(), 32);
            assert_pcm(
                &render(&mut old, 1024, 63),
                &render(&mut oracle, 1024, 256),
                1e-7,
            );
            continue;
        }
        expected.try_inherit_state_from(&mut oracle).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        assert_pcm(
            &render(&mut next, 1024, 63),
            &render(&mut expected, 1024, 256),
            1e-7,
        );
        assert!(
            next.mutable_instruments[1]
                .as_ref()
                .unwrap()
                .proofs()
                .next()
                .is_some()
        );
    }
}

#[test]
#[ignore = "release MIDI throughput evidence; run explicitly with --release --ignored --nocapture"]
fn profile_real_midi_routed_graph() {
    use std::time::Instant;
    for routed in [false, true] {
        let mut f = fixture(routed);
        for &clip in &f.clips {
            f.editor
                .execute(Command::SetSessionMidiClipSource {
                    clip,
                    source_length: beat(512),
                    source_start: BeatTime::ZERO,
                    loop_start: BeatTime::ZERO,
                    loop_end: beat(512),
                    looping: true,
                })
                .unwrap();
        }
        let mut runtime = start(
            prepare(&f, None),
            if routed { &f.clips[..1] } else { &f.clips[1..] },
        );
        let mut block = [[0.0; 2]; 256];
        for _ in 0..32 {
            runtime.render(&mut block).unwrap();
        }
        let mut samples = [0u128; 101];
        for sample in &mut samples {
            let start = Instant::now();
            for _ in 0..16 {
                runtime.render(&mut block).unwrap();
                std::hint::black_box(&block);
            }
            *sample = start.elapsed().as_nanos();
        }
        samples.sort_unstable();
        let frames = 16.0 * 256.0;
        println!(
            "midi_{}: rate=48000 tracks=3 destinations=2 algorithms=sine+fm4 block=256 source_loop=512 notes_per_period=2 warmup=8192 samples=101 median_ns_per_frame={:.2} p99_ns_per_frame={:.2} median_frames_per_second={:.0}",
            if routed { "routed" } else { "local" },
            samples[50] as f64 / frames,
            samples[99] as f64 / frames,
            frames * 1e9 / samples[50] as f64
        );
    }
}

#[path = "../../examples/support/latency_fixture.rs"]
mod latency_fixture;

fn prepare_with_delay(f: &Fixture) -> Arc<PreparedRevision> {
    let definitions: [&dyn hex_lattice::NativeDeviceDefinition; 1] = [&latency_fixture::Definition];
    let effects = hex_lattice::NativeDeviceRegistry::new(&definitions).unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    Arc::new(
        PreparedRevision::prepare_with_registries(
            f.editor.project(),
            &f.media,
            Arc::new(hex_devices::SineTable::new()),
            None,
            &effects,
            &instruments,
        )
        .unwrap(),
    )
}

#[test]
fn positive_delay_midi_routes_align_pcm_capture_and_source_launch_lifecycle() {
    use hex_lattice_contract::NativeDeviceMetadata;
    let mut delayed = fixture(true);
    let zero = prepare(&delayed, None);
    delayed
        .editor
        .execute(Command::InsertDevice {
            track: delayed.tracks[1],
            before: None,
            state: latency_fixture::Definition.default_state().unwrap(),
        })
        .unwrap();
    let revision = prepare_with_delay(&delayed);
    assert_eq!(revision.arrangement().main_latency_frames(), 7);
    for partition in [1, 63, 256, 513] {
        for session in [false, true] {
            let clips = if session { &delayed.clips[..1] } else { &[] };
            let mut actual = start(Arc::clone(&revision), clips);
            let mut reference = start(Arc::clone(&zero), clips);
            let mut expected = vec![[0.0; 2]; 7];
            expected.extend(render(&mut reference, 320, 256));
            assert_pcm(&render(&mut actual, 327, partition), &expected, 1e-7);
        }
        let mut actual = start(Arc::clone(&revision), &delayed.clips[..1]);
        let mut reference = start(Arc::clone(&zero), &delayed.clips[..1]);
        let mut pcm = render(&mut actual, 63, partition);
        let mut expected = vec![[0.0; 2]; 7];
        expected.extend(render(&mut reference, 63, 256));
        for runtime in [&mut actual, &mut reference] {
            runtime
                .apply(SessionPlaybackCommand::StopTrack {
                    track: delayed.tracks[0],
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        pcm.extend(render(&mut actual, 64, partition));
        expected.extend(render(&mut reference, 64, 256));
        for runtime in [&mut actual, &mut reference] {
            runtime
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip: delayed.clips[0],
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        pcm.extend(render(&mut actual, 327, partition));
        expected.extend(render(&mut reference, 320, 256));
        assert_pcm(&pcm, &expected, 1e-7);
    }
    let mut captured = start(Arc::clone(&revision), &[]);
    let plan = captured
        .output_capture_plan(
            &[
                OutputCaptureId::Track(delayed.tracks[1]),
                OutputCaptureId::Track(delayed.tracks[2]),
            ],
            327,
        )
        .unwrap();
    let mut main = vec![[0.0; 2]; 327];
    let mut a = main.clone();
    let mut b = main.clone();
    let mut outputs = [&mut a[..], &mut b[..]];
    captured
        .render_with_capture(&mut main, &mut OutputCapture::new(&plan, &mut outputs))
        .unwrap();
    for i in 0..main.len() {
        assert_eq!(
            main[i],
            [a[i][0] + b[i][0], a[i][1] + b[i][1]],
            "capture at {i}"
        );
    }
    assert!(main[..7].iter().all(|frame| *frame == [0.0; 2]));
    let mut reference = start(zero, &[]);
    let mut expected = vec![[0.0; 2]; 7];
    expected.extend(render(&mut reference, 320, 256));
    assert!(expected.iter().flatten().any(|sample| *sample != 0.0));
    assert_pcm(&main, &expected, 1e-7);
}
