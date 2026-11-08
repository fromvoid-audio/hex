use super::*;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterId};
use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue};

fn fixture() -> (Editor, hex_project::TrackId, hex_project::DeviceId) {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "frozen fm4".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_devices::fm4::device::FM4_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(4).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 120,
        })
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: ParameterId::from_static(29),
            points: vec![
                point(BeatTime::ZERO, 0.75),
                point(BeatTime::from_raw(BeatTime::ONE.raw() / 128), 0.9),
            ],
        })
        .unwrap();
    (editor, track, device)
}
fn point(beat: BeatTime, value: f32) -> AutomationPoint {
    AutomationPoint {
        beat,
        value: AutomationValue::new(value).unwrap(),
        outgoing: AutomationInterpolation::Step,
    }
}
fn prepared(editor: &Editor) -> Arc<PreparedRevision> {
    crate::device_automation::with_fm4_source_curves(|| {
        Arc::new(
            PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::new(crate::SineTable::new()),
                None,
            )
            .unwrap(),
        )
    })
}
fn playback(editor: &Editor) -> SessionPlayback {
    SessionPlayback::new(prepared(editor)).unwrap()
}
fn render(runtime: &mut SessionPlayback, count: usize, partition: usize) -> Vec<Frame> {
    let mut output = vec![[0.0; 2]; count];
    for block in output.chunks_mut(partition) {
        runtime.render(block).unwrap();
    }
    output
}
#[test]
fn source_curve_fixture_scope_is_narrow_and_restores_on_unwind() {
    let (mut editor, _, device) = fixture();
    assert!(crate::device_automation::validate_project(editor.project()).is_err());
    crate::device_automation::with_fm4_source_curves(|| {
        crate::device_automation::validate_project(editor.project()).unwrap();
        editor
            .execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: ParameterId::from_static(1),
                points: vec![point(BeatTime::ZERO, 0.5)],
            })
            .unwrap();
        assert!(crate::device_automation::validate_project(editor.project()).is_err());
    });
    assert!(crate::device_automation::validate_project(editor.project()).is_err());
    let _ = std::panic::catch_unwind(|| {
        crate::device_automation::with_fm4_source_curves(|| panic!("scope restoration"))
    });
    assert!(crate::device_automation::validate_project(fixture().0.project()).is_err());
}
#[test]
fn detached_fm4_freezes_latest_target_across_pause_bypass_removal_and_revision() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            for paused in [false, true] {
                for removed in [false, true] {
                    let (mut editor, track, device) = fixture();
                    let mut old = playback(&editor);
                    let mut reference = playback(&editor);
                    for runtime in [&mut old, &mut reference] {
                        runtime.apply(SessionPlaybackCommand::Play).unwrap();
                        render(runtime, 20, 20);
                        // A planning observation must not freeze the eventual value.
                        runtime.mutable_instruments[0]
                            .as_ref()
                            .unwrap()
                            .preflight_tail_parameters()
                            .unwrap();
                        render(runtime, 53, 7);
                    }
                    let mut oracle_editor = Editor::new(editor.project().clone(), 16).unwrap();
                    oracle_editor
                        .execute(Command::ReplaceDeviceAutomation {
                            device,
                            parameter: ParameterId::from_static(29),
                            points: vec![point(BeatTime::ZERO, 0.9)],
                        })
                        .unwrap();
                    let mut oracle = playback(&oracle_editor);
                    oracle.try_inherit_state_from(&mut reference).unwrap();
                    oracle.apply(SessionPlaybackCommand::Pause).unwrap();
                    if paused {
                        old.apply(SessionPlaybackCommand::Pause).unwrap();
                    }
                    if removed {
                        editor.execute(Command::RemoveTrack(track)).unwrap();
                    } else {
                        editor
                            .execute(Command::SetDeviceBypass {
                                device,
                                bypassed: true,
                            })
                            .unwrap();
                    }
                    let mut actual = playback(&editor);
                    actual.try_inherit_state_from(&mut old).unwrap();
                    assert_eq!(actual.tail_mutable_instruments.iter().flatten().count(), 1);
                    // A refused tail render must restore the borrowed frame before
                    // rollback; the retry below must match uninterrupted audio.
                    let tail = actual
                        .tail_mutable_instruments
                        .iter_mut()
                        .flatten()
                        .next()
                        .unwrap();
                    tail.runtime.begin_block();
                    let failed = tail
                        .runtime
                        .with_frozen_parameters(|runtime, values| {
                            runtime.process_frozen_tail_segment(
                                Some(tail.track_id),
                                u64::MAX,
                                &mut [[0.0; 2]; 2],
                                &mut Default::default(),
                                &mut Vec::new(),
                                None,
                                values,
                            )
                        })
                        .unwrap();
                    assert!(failed.is_err());
                    tail.runtime.rollback_block();
                    let expected = render(&mut oracle, 113, 113);
                    let output = render(&mut actual, 113, 3);
                    assert_eq!(output, expected, "paused={paused} removed={removed}");
                    assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
                    // A further canonical revision carries the existing tail frame.
                    let mut successor = playback(&editor);
                    successor.try_inherit_state_from(&mut actual).unwrap();
                    let expected = render(&mut oracle, 9000, 4097);
                    assert_eq!(render(&mut successor, 9000, 31), expected);
                    assert!(expected[8000..].iter().all(|frame| *frame == [0.0; 2]));
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn second_capture_refusal_preserves_both_donor_voices_and_retry() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let (mut editor, track, _) = fixture();
            editor
                .execute(Command::DuplicateTracks {
                    tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
                    without_clips: false,
                })
                .unwrap();
            let mut old = playback(&editor);
            let mut reference = playback(&editor);
            for runtime in [&mut old, &mut reference] {
                runtime.apply(SessionPlaybackCommand::Play).unwrap();
                render(runtime, 73, 7);
            }
            let tracks: Vec<_> = editor
                .project()
                .tracks()
                .iter()
                .map(|track| track.id())
                .collect();
            for track in tracks {
                editor.execute(Command::RemoveTrack(track)).unwrap();
            }
            let mut candidate = playback(&editor);
            crate::mutable_instrument::frozen_parameters::with_capture_failure(1, || {
                assert!(candidate.try_inherit_state_from(&mut old).is_err());
            });
            assert_eq!(old.mutable_instruments.iter().flatten().count(), 2);
            assert!(
                candidate
                    .tail_mutable_instruments
                    .iter()
                    .all(Option::is_none)
            );
            assert_eq!(render(&mut old, 97, 7), render(&mut reference, 97, 97));
            // Existing publication candidates are single-use after refusal.
            let mut candidate = playback(&editor);
            candidate.try_inherit_state_from(&mut old).unwrap();
            assert_eq!(
                candidate.tail_mutable_instruments.iter().flatten().count(),
                2
            );
            assert!(
                render(&mut candidate, 64, 3)
                    .iter()
                    .flatten()
                    .any(|sample| sample.abs() > 1e-6)
            );
        })
        .unwrap()
        .join()
        .unwrap();
}
