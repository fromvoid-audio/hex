//! Real canonical device preparation, publication and PCM; no injected provider.
use super::*;
use hex_devices::operator_state::{harmonic_parameter, parameter};
use hex_lattice_contract::{ParameterId, ParameterValue};

fn carrier_state() -> hex_lattice_contract::NativeDeviceState {
    let mut state = crate::operator_legacy_test::legacy_operator_state();
    for operator in 1..=4 {
        state = set(
            &state,
            parameter::oscillator_level(operator),
            ParameterValue::Float(if operator == 1 { 1.0 } else { 0.0 }),
        );
        state = set(
            &state,
            parameter::carrier(operator),
            ParameterValue::Float(if operator == 1 { 1.0 } else { 0.0 }),
        );
        for source in 1..=4 {
            if source != operator {
                state = set(
                    &state,
                    parameter::modulation(source, operator),
                    ParameterValue::Float(0.0),
                );
            }
        }
        for field in [
            parameter::EnvelopeField::Initial,
            parameter::EnvelopeField::Peak,
            parameter::EnvelopeField::Sustain,
        ] {
            state = set(
                &state,
                parameter::envelope(operator, field),
                ParameterValue::Float(1.0),
            );
        }
    }
    state
}

fn set(
    state: &hex_lattice_contract::NativeDeviceState,
    id: ParameterId,
    value: ParameterValue,
) -> hex_lattice_contract::NativeDeviceState {
    crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        state,
        id,
        value,
    )
    .unwrap()
}
fn fixture(notes: usize, duration: u64) -> Project {
    let mut snapshot = project(0.5, 0, duration).snapshot();
    let state = carrier_state();
    snapshot.tracks[0].instrument.as_mut().unwrap().state = state;
    let authored = |first: u64| {
        (0..notes)
            .map(|index| MidiNoteSnapshot {
                active: true,
                id: first + index as u64,
                start_raw: 0,
                duration_raw: duration,
                // Keep all 64 simultaneous voices on distinct authored pitches.
                pitch: 36 + index as u8,
                channel: 0,
                velocity: 100,
            })
            .collect::<Vec<_>>()
    };
    if let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut snapshot.tracks[0].content {
        midi_clips[0].notes = authored(20);
    } else if let TrackContentSnapshot::Instrument(clips) = &mut snapshot.tracks[0].content {
        clips[0].notes = authored(20);
    }
    if let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content {
        clip.source.notes = authored(100);
    }
    snapshot.next_id = 300;
    Project::from_legacy_snapshot(snapshot).unwrap()
}
fn edit(project: &Project, changes: &[(ParameterId, ParameterValue)]) -> Project {
    edit_track(project, project.tracks()[0].id(), changes)
}
fn edit_track(
    project: &Project,
    track: hex_project::TrackId,
    changes: &[(ParameterId, ParameterValue)],
) -> Project {
    let device = project.track(track).unwrap().instrument().unwrap();
    let mut state = device.state().clone();
    for (id, value) in changes {
        state = set(&state, *id, *value);
    }
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: device.id(),
            state,
        })
        .unwrap();
    editor.project().clone()
}

fn routed_session_fixture() -> (
    Project,
    hex_project::TrackId,
    hex_project::TrackId,
    hex_project::ClipId,
) {
    use hex_project::{Command, Created, Editor};

    let beat = |raw| hex_project::BeatTime::from_raw(raw);
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(source) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("source track")
    };
    let Created::Track(destination) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "destination".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("destination track")
    };
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let mut clips = Vec::new();
    for track in [source, destination] {
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "notes".into(),
                source_length: beat(8 << 32),
            })
            .unwrap()
        else {
            panic!("session clip")
        };
        clips.push(clip);
        for (duration, pitch) in [(1 << 26, 60), (8 << 32, 64)] {
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: beat(0),
                    duration: beat(duration),
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
    }
    editor
        .execute(Command::RemoveInstrument { track: source })
        .unwrap();
    let device = editor
        .project()
        .track(destination)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: carrier_state(),
        })
        .unwrap();
    editor
        .execute(Command::SetMidiSource {
            destination,
            source: Some(source),
        })
        .unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 64,
            enabled: true,
        })
        .unwrap();
    (editor.project().clone(), source, destination, clips[0])
}
fn prepared(
    project: &Project,
    previous: Option<&PreparedSessionInventory>,
) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(
            project,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            previous,
        )
        .unwrap(),
    )
}
fn start(project: &Project, session: bool) -> SessionPlayback {
    let mut runtime = SessionPlayback::new(prepared(project, None)).unwrap();
    if session {
        let scene = runtime.revision.session().scenes()[0].id;
        runtime
            .apply(SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
    }
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    runtime
}
#[test]
fn first_paint_preserves_all_64_held_or_released_analytic_voices_and_undo_stays_live() {
    for session in [false, true] {
        for released in [false, true] {
            let original = fixture(64, if released { 1 << 26 } else { 8 << 32 });
            let mut old = start(&original, session);
            let mut oracle = start(&original, session);
            old.render(&mut [[0.0; 2]; 512]).unwrap();
            oracle.render(&mut [[0.0; 2]; 512]).unwrap();
            let custom = edit(
                &original,
                &[
                    (harmonic_parameter(1, 1), ParameterValue::Float(0.0)),
                    (harmonic_parameter(1, 2), ParameterValue::Float(1.0)),
                ],
            );
            let mut next =
                SessionPlayback::new(prepared(&custom, Some(old.revision.session()))).unwrap();
            assert_eq!(
                next.try_inherit_state_from(&mut old),
                Ok(SessionHandoffReport {
                    cancelled_pending: [None; MAX_TRACKS],
                    cancelled_pending_len: 0
                })
            );
            assert_eq!(
                next.mutable_instruments[0]
                    .as_ref()
                    .unwrap()
                    .proofs()
                    .count(),
                64
            );
            let mut actual = [[0.0; 2]; 1];
            let mut expected = actual;
            next.render(&mut actual).unwrap();
            oracle.render(&mut expected).unwrap();
            for (a, b) in actual[0].iter().zip(expected[0]) {
                assert!((a - b).abs() < 2e-5, "source boundary {a} != {b}");
            }
            next.render(&mut [[0.0; 2]; 37]).unwrap();
            // Undo to never-authored state retains the compiled continuation class;
            // no restart, quiescence wait, or serialized sticky flag is needed.
            let mut undone =
                SessionPlayback::new(prepared(&original, Some(next.revision.session()))).unwrap();
            undone.try_inherit_state_from(&mut next).unwrap();
            assert_eq!(
                undone.mutable_instruments[0]
                    .as_ref()
                    .unwrap()
                    .proofs()
                    .count(),
                64
            );
            undone.render(&mut [[0.0; 2]; 257]).unwrap();
        }
    }
}

#[test]
fn routed_session_owner_imports_held_and_releasing_notes_through_first_paint_and_undo() {
    // Retain every real oracle/transition owner, but keep their storage out of
    // the caller's debug frame. The production constructor still runs normally
    // inside this small test-only boundary; no thread stack is enlarged.
    fn playback(revision: Arc<PreparedRevision>) -> Box<SessionPlayback> {
        Box::new(SessionPlayback::new(revision).unwrap())
    }
    let (original, source, destination, source_clip) = routed_session_fixture();
    let revision = prepared(&original, None);
    let mut old = playback(Arc::clone(&revision));
    let mut old_reference = playback(Arc::clone(&revision));
    let mut analytic_oracle = playback(revision);
    for runtime in [&mut old, &mut old_reference, &mut analytic_oracle] {
        runtime
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: source_clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut old_pcm = [[0.0; 2]; 512];
    old.render(&mut old_pcm).unwrap();
    old_reference.render(&mut [[0.0; 2]; 512]).unwrap();
    analytic_oracle.render(&mut [[0.0; 2]; 512]).unwrap();
    assert!(old.arrangement_loop_pass > 0);
    assert!(old_pcm.iter().flatten().any(|sample| sample.abs() > 0.01));

    let custom = edit_track(
        &original,
        destination,
        &[
            (harmonic_parameter(1, 1), ParameterValue::Float(0.0)),
            (harmonic_parameter(1, 2), ParameterValue::Float(1.0)),
        ],
    );
    let candidate = prepared(&custom, Some(old.revision.session()));
    let mut painted = playback(Arc::clone(&candidate));
    painted.try_inherit_state_from(&mut old).unwrap();
    let mut painted_reference = playback(candidate);
    painted_reference
        .try_inherit_state_from(&mut old_reference)
        .unwrap();
    assert_eq!(
        painted.mutable_instruments[1]
            .as_ref()
            .unwrap()
            .proofs()
            .count(),
        2,
        "the destination imports its effective Session owner's held and releasing notes"
    );
    assert!(
        painted.mutable_instruments[1]
            .as_ref()
            .unwrap()
            .proofs()
            .all(|proof| proof.signature.key().loop_pass == 0)
    );
    let mut actual = [[0.0; 2]; 1];
    let mut expected = actual;
    painted.render(&mut actual).unwrap();
    analytic_oracle.render(&mut expected).unwrap();
    for (a, b) in actual[0].iter().zip(expected[0]) {
        assert!((a - b).abs() < 2e-5, "routed first paint {a} != {b}");
    }
    painted_reference.render(&mut [[0.0; 2]; 1]).unwrap();

    assert_eq!(
        painted.apply(SessionPlaybackCommand::StopTrack {
            track: source,
            quantization: LaunchQuantization::Immediate,
        }),
        Ok(()),
        "the source route cuts its authored destination gates without resetting source DSP"
    );
    // Independent routing oracle: explicitly stop the destination and source.
    for track in [destination, source] {
        painted_reference
            .apply(SessionPlaybackCommand::StopTrack {
                track,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
    }
    let mut actual = [[0.0; 2]; 37];
    let mut expected = actual;
    painted.render(&mut actual).unwrap();
    painted_reference.render(&mut expected).unwrap();
    assert_eq!(actual, expected);

    let mut undone = playback(prepared(&original, Some(painted.revision.session())));
    undone.try_inherit_state_from(&mut painted).unwrap();
    let mut undone_reference = playback(prepared(
        &original,
        Some(painted_reference.revision.session()),
    ));
    undone_reference
        .try_inherit_state_from(&mut painted_reference)
        .unwrap();
    assert_eq!(
        undone.mutable_instruments[1]
            .as_ref()
            .unwrap()
            .proofs()
            .count(),
        2
    );
    let mut actual = [[0.0; 2]; 37];
    let mut expected = actual;
    undone.render(&mut actual).unwrap();
    undone_reference.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}
#[test]
fn range_masks_cached_partial_64_and_live_reversals_are_partition_invariant() {
    let original = fixture(1, 8 << 32);
    let custom = edit(
        &original,
        &[
            (harmonic_parameter(1, 1), ParameterValue::Float(0.0)),
            (harmonic_parameter(1, 64), ParameterValue::Float(1.0)),
        ],
    );
    let hidden = edit(
        &custom,
        &[(
            hex_devices::operator_state::harmonic_range_parameter(1),
            ParameterValue::Choice(0),
        )],
    );
    assert_eq!(
        crate::native_device_view_for_class(
            crate::NativeDeviceClass::Instrument,
            hidden.tracks()[0].instrument().unwrap().state()
        )
        .unwrap()
        .parameters
        .iter()
        .find(|p| p.descriptor.id == harmonic_parameter(1, 64))
        .unwrap()
        .value,
        ParameterValue::Float(1.0)
    );
    let mut a = start(&custom, false);
    let mut b = start(&custom, false);
    let mut sounding = [[0.0; 2]; 512];
    a.render(&mut sounding).unwrap();
    b.render(&mut [[0.0; 2]; 512]).unwrap();
    assert!(sounding.iter().flatten().any(|x| x.abs() > 0.01));
    let mut next_a = SessionPlayback::new(prepared(&hidden, Some(a.revision.session()))).unwrap();
    let mut next_b = SessionPlayback::new(prepared(&hidden, Some(b.revision.session()))).unwrap();
    next_a.try_inherit_state_from(&mut a).unwrap();
    next_b.try_inherit_state_from(&mut b).unwrap();
    let mut whole = [[0.0; 2]; 512];
    let mut split = whole;
    next_a.render(&mut whole).unwrap();
    for block in split.chunks_mut(17) {
        next_b.render(block).unwrap();
    }
    assert_eq!(whole, split);
    assert!(whole[240..].iter().flatten().all(|x| *x == 0.0));
}

#[test]
fn first_paint_imports_feedback_and_lfo_recurrence_and_preserves_enable_clock() {
    for lfo in [false, true] {
        let original = fixture(16, 8 << 32);
        let original = edit(
            &original,
            &if lfo {
                vec![
                    (ParameterId::from_static(82), ParameterValue::Bool(true)),
                    (ParameterId::from_static(87), ParameterValue::Float(0.2)),
                ]
            } else {
                vec![(parameter::feedback(1), ParameterValue::Float(0.2))]
            },
        );
        let mut old = start(&original, false);
        let mut oracle = start(&original, false);
        old.render(&mut [[0.0; 2]; 512]).unwrap();
        oracle.render(&mut [[0.0; 2]; 512]).unwrap();
        let custom = edit(
            &original,
            &[(harmonic_parameter(1, 2), ParameterValue::Float(0.5))],
        );
        let mut next =
            SessionPlayback::new(prepared(&custom, Some(old.revision.session()))).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut a = [[0.0; 2]; 1];
        let mut b = a;
        next.render(&mut a).unwrap();
        oracle.render(&mut b).unwrap();
        assert_eq!(
            a, b,
            "legacy recurrence must be exact at the source boundary"
        );
        // A harmonic change during an enable ramp must not restart its clock.
        let disabled = edit(
            &custom,
            &[(parameter::enabled(1), ParameterValue::Bool(false))],
        );
        let mut toggled =
            SessionPlayback::new(prepared(&disabled, Some(next.revision.session()))).unwrap();
        toggled.try_inherit_state_from(&mut next).unwrap();
        toggled.render(&mut [[0.0; 2]; 37]).unwrap();
        let edited = edit(
            &disabled,
            &[(harmonic_parameter(1, 3), ParameterValue::Float(0.25))],
        );
        let mut painted =
            SessionPlayback::new(prepared(&edited, Some(toggled.revision.session()))).unwrap();
        painted.try_inherit_state_from(&mut toggled).unwrap();
        let mut pcm = [[0.0; 2]; 512];
        painted.render(&mut pcm).unwrap();
        assert!(pcm[203..].iter().flatten().all(|v| *v == 0.0));
    }
}

#[test]
fn first_paint_capacity_refusal_preserves_old_committed_audio_and_all_occurrences() {
    let original = fixture(64, 8 << 32);
    let mut old = start(&original, false);
    let mut oracle = start(&original, false);
    old.render(&mut [[0.0; 2]; 512]).unwrap();
    oracle.render(&mut [[0.0; 2]; 512]).unwrap();
    let custom = edit(
        &original,
        &[
            (harmonic_parameter(1, 2), ParameterValue::Float(0.5)),
            (parameter::feedback(1), ParameterValue::Float(0.2)),
        ],
    );
    assert!(
        PreparedRevision::prepare(
            &custom,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            Some(old.revision.session())
        )
        .is_err(),
        "64 live voices cannot fit the feedback class's 16-voice admission"
    );
    let mut actual = [[0.0; 2]; 512];
    let mut expected = actual;
    old.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn first_paint_keeps_active_clean_filter_integrators_for_64_voices_and_feedback_lfo() {
    for (feedback, lfo, notes) in [(false, false, 64), (true, false, 16), (true, true, 16)] {
        let original = fixture(notes, 8 << 32);
        let mut changes = vec![
            (ParameterId::from_static(20001), ParameterValue::Bool(true)),
            (
                ParameterId::from_static(20005),
                ParameterValue::Float(650.0),
            ),
        ];
        if feedback {
            changes.push((parameter::feedback(1), ParameterValue::Float(0.2)));
        }
        if lfo {
            changes.extend([
                (ParameterId::from_static(82), ParameterValue::Bool(true)),
                (ParameterId::from_static(87), ParameterValue::Float(25.0)),
            ]);
        }
        let original = edit(&original, &changes);
        let mut old = start(&original, false);
        let mut oracle = start(&original, false);
        old.render(&mut [[0.0; 2]; 512]).unwrap();
        oracle.render(&mut [[0.0; 2]; 512]).unwrap();
        let custom = edit(
            &original,
            &[(harmonic_parameter(1, 2), ParameterValue::Float(0.5))],
        );
        let mut next =
            SessionPlayback::new(prepared(&custom, Some(old.revision.session()))).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut a = [[0.0; 2]; 1];
        let mut b = a;
        next.render(&mut a).unwrap();
        oracle.render(&mut b).unwrap();
        assert_eq!(a, b, "first paint must preserve the actual filter history");
        next.render(&mut [[0.0; 2]; 37]).unwrap();
        // Mid-ramp filter and harmonic changes retain the independent carrier integrators.
        let changed = edit(
            &custom,
            &[
                (
                    ParameterId::from_static(20005),
                    ParameterValue::Float(3000.0),
                ),
                (harmonic_parameter(1, 3), ParameterValue::Float(0.25)),
            ],
        );
        let mut more =
            SessionPlayback::new(prepared(&changed, Some(next.revision.session()))).unwrap();
        more.try_inherit_state_from(&mut next).unwrap();
        let mut actual = [[0.0; 2]; 1];
        let mut expected = actual;
        more.render(&mut actual).unwrap();
        next.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
        more.render(&mut [[0.0; 2]; 257]).unwrap();
    }
}

#[path = "harmonic_tests/cue.rs"]
mod cue;

#[test]
fn clip_gain_handoff_keeps_active_native_voice_in_both_domains() {
    for session in [false, true] {
        let original = fixture(1, 8 << 32);
        let clip = if session {
            prepared(&original, None).session().midi_clips()[0].id
        } else {
            original.tracks()[0].midi_clips()[0].id()
        };
        let mut old = start(&original, session);
        let mut oracle = start(&original, session);
        old.render(&mut [[0.0; 2]; 128]).unwrap();
        oracle.render(&mut [[0.0; 2]; 128]).unwrap();
        let gain = hex_project::ClipGain::new(-6_000).unwrap();
        let mut editor = Editor::new(original, 8).unwrap();
        editor
            .execute(Command::SetMidiClipGain { clip, gain })
            .unwrap();
        let mut next =
            SessionPlayback::new(prepared(editor.project(), Some(old.revision.session()))).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = [[0.0; 2]; 128];
        let mut expected = actual;
        next.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert!(expected.iter().flatten().any(|v| v.abs() > 1e-4));
        for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            for channel in 0..2 {
                assert!(
                    (actual[channel] - expected[channel] * gain.linear()).abs() < 1e-6,
                    "session={session} frame={index} channel={channel}"
                );
            }
        }
    }
}

#[test]
fn native_occurrence_gain_accepts_clip_and_curve_product_in_both_domains() {
    use hex_project::{ClipGain, MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
    for session in [false, true] {
        let render = |mode: u8| {
            let mut editor = Editor::new(fixture(1, 8 << 32), 8).unwrap();
            let clip = if session {
                prepared(editor.project(), None).session().midi_clips()[0].id
            } else {
                editor.project().tracks()[0].midi_clips()[0].id()
            };
            if mode == 2 {
                let length = editor.project().midi_source(clip).unwrap().source_length();
                editor
                    .execute(Command::ReplaceMidiClipVolume {
                        clip,
                        volume: MidiClipVolume::new(
                            vec![MidiClipVolumePoint::linear(
                                BeatTime::ZERO,
                                MidiClipGain::new(4.0).unwrap(),
                            )],
                            length,
                        )
                        .unwrap(),
                    })
                    .unwrap();
            }
            if mode != 0 {
                editor
                    .execute(Command::SetMidiClipGain {
                        clip,
                        gain: ClipGain::new(24_000).unwrap(),
                    })
                    .unwrap();
            }
            let mut playback = start(editor.project(), session);
            let mut output = [[0.0; 2]; 256];
            for block in output.chunks_mut(17) {
                playback.render(block).unwrap();
            }
            output
        };
        let unity = render(0);
        assert!(unity.iter().flatten().any(|v| v.abs() > 1.0e-4));
        for mode in [1, 2] {
            let gained = render(mode);
            let multiplier = f64::from(ClipGain::new(24_000).unwrap().linear())
                * if mode == 2 { 4.0 } else { 1.0 };
            for (index, (actual, reference)) in gained.iter().zip(unity).enumerate() {
                for channel in 0..2 {
                    assert!(
                        (f64::from(actual[channel]) - f64::from(reference[channel]) * multiplier)
                            .abs()
                            < 1.0e-5,
                        "session={session} frame={index} channel={channel}"
                    );
                }
            }
        }
    }
}

#[test]
fn first_harmonic_edit_preserves_live_clip_volume_ramp_in_arrangement_and_console() {
    use hex_project::{MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
    for session in [false, true] {
        let run = |volume: bool| {
            let mut editor = Editor::new(fixture(1, 8 << 32), 8).unwrap();
            let clip = if session {
                prepared(editor.project(), None).session().midi_clips()[0].id
            } else {
                editor.project().tracks()[0].midi_clips()[0].id()
            };
            if volume {
                let length = editor.project().midi_source(clip).unwrap().source_length();
                editor
                    .execute(Command::ReplaceMidiClipVolume {
                        clip,
                        volume: MidiClipVolume::new(
                            vec![
                                MidiClipVolumePoint::linear(
                                    BeatTime::ZERO,
                                    MidiClipGain::new(0.25).unwrap(),
                                ),
                                MidiClipVolumePoint::linear(
                                    BeatTime::ONE,
                                    MidiClipGain::new(0.75).unwrap(),
                                ),
                            ],
                            length,
                        )
                        .unwrap(),
                    })
                    .unwrap();
            }
            let mut old = start(editor.project(), session);
            old.render(&mut [[0.0; 2]; 64]).unwrap();
            let custom = edit(
                editor.project(),
                &[
                    (harmonic_parameter(1, 1), ParameterValue::Float(0.0)),
                    (harmonic_parameter(1, 2), ParameterValue::Float(1.0)),
                ],
            );
            let mut next =
                SessionPlayback::new(prepared(&custom, Some(old.revision.session()))).unwrap();
            next.try_inherit_state_from(&mut old).unwrap();
            assert_eq!(
                next.mutable_instruments[0]
                    .as_ref()
                    .unwrap()
                    .proofs()
                    .count(),
                1
            );
            let mut output = [[0.0; 2]; 512];
            for block in output.chunks_mut(17) {
                next.render(block).unwrap();
            }
            output
        };
        let baseline = run(false);
        let actual = run(true);
        assert!(baseline.iter().flatten().any(|v| v.abs() > 1e-3));
        for (i, (a, b)) in actual.iter().zip(baseline).enumerate() {
            let gain = 0.25 + 0.5 * (i + 64) as f64 / 24_000.0;
            for c in 0..2 {
                assert!(
                    (f64::from(a[c]) - f64::from(b[c]) * gain).abs() < 2e-7,
                    "session={session} frame={i} actual={a:?} unity={b:?} gain={gain}"
                );
            }
        }
    }
}

#[test]
fn arrangement_clip_gain_uses_content_clock_after_looped_tempo_handoff() {
    use hex_project::{MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
    let run = |volume: bool| {
        let source = edit(
            &fixture(1, 1 << 22),
            &[
                (harmonic_parameter(1, 1), ParameterValue::Float(0.0)),
                (harmonic_parameter(1, 2), ParameterValue::Float(1.0)),
            ],
        );
        let clip = source.tracks()[0].midi_clips()[0].id();
        let mut snapshot = source.snapshot();
        snapshot.loop_region = Some(LoopRegion::new(0, 24_000, true).unwrap());
        let mut editor = Editor::new(Project::from_legacy_snapshot(snapshot).unwrap(), 8).unwrap();
        if volume {
            let length = editor.project().midi_source(clip).unwrap().source_length();
            editor
                .execute(Command::ReplaceMidiClipVolume {
                    clip,
                    volume: MidiClipVolume::new(
                        vec![MidiClipVolumePoint::linear(
                            BeatTime::ZERO,
                            MidiClipGain::new(0.25).unwrap(),
                        )],
                        length,
                    )
                    .unwrap(),
                })
                .unwrap();
        }
        let mut old = start(editor.project(), false);
        old.render(&mut vec![[0.0; 2]; 60_000]).unwrap();
        assert_eq!(old.arrangement_frame(), 12_000);
        assert!(
            old.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .committed_is_quiescent()
        );
        let mut snapshot = editor.project().snapshot();
        snapshot.tempo_points = vec![TempoPoint::new(
            BeatTime::ZERO,
            Tempo::new(90_000_000).unwrap(),
        )];
        let changed = Project::from_legacy_snapshot(snapshot).unwrap();
        let mut next =
            SessionPlayback::new(prepared(&changed, Some(old.revision.session()))).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        assert!(next.clock_anchor().is_some());
        let mut output = vec![[0.0; 2]; 24_000];
        for block in output.chunks_mut(17) {
            next.render(block).unwrap();
        }
        output
    };
    let baseline = run(false);
    let actual = run(true);
    assert!(baseline.iter().flatten().any(|v| v.abs() > 1e-3));
    for (a, b) in actual.iter().zip(baseline) {
        for c in 0..2 {
            assert!((a[c] - b[c] * 0.25).abs() < 2e-7);
        }
    }
}

#[test]
fn first_track_limiter_keeps_held_mutable_voice_and_its_note_off() {
    let original = edit(
        &fixture(1, 1 << 28),
        &[(harmonic_parameter(1, 2), ParameterValue::Float(0.1))],
    );
    let mut donor = start(&original, false);
    let mut oracle = start(&original, false);
    for runtime in [&mut donor, &mut oracle] {
        runtime.render(&mut [[0.0; 2]; 257]).unwrap();
        assert_eq!(
            runtime.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .proofs()
                .count(),
            1
        );
    }
    let mut editor = Editor::new(original, 8).unwrap();
    editor
        .execute(Command::InsertDevice {
            track: editor.project().tracks()[0].id(),
            before: None,
            state: crate::default_native_device_state_for_class(
                crate::NativeDeviceClass::Effect,
                hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
            )
            .unwrap(),
        })
        .unwrap();
    let revision = prepared(editor.project(), Some(donor.revision.session()));
    let delay = revision.arrangement().main_latency_frames() as usize;
    let mut candidate = SessionPlayback::new(revision).unwrap();
    candidate.try_inherit_state_from(&mut donor).unwrap();
    assert_eq!(candidate.performance_frame(), donor.performance_frame());
    assert_eq!(
        candidate.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .proofs()
            .count(),
        1
    );
    let mut actual = vec![[0.0; 2]; delay + 2048];
    let mut expected = actual.clone();
    oracle.render(&mut expected[delay..]).unwrap();
    for block in actual.chunks_mut(63) {
        candidate.render(block).unwrap();
    }
    assert_eq!(actual, expected);
    assert!(
        candidate.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .proofs()
            .all(|proof| proof.note_off_sent)
    );
}
