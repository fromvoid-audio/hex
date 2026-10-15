//! Real canonical device preparation, publication and PCM; no injected provider.
use super::*;
use hex_devices::operator_state::{OPERATOR_TYPE_ID, harmonic_parameter, parameter};
use hex_lattice_contract::{ParameterId, ParameterValue};

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
    let mut state = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Instrument,
        OPERATOR_TYPE_ID,
    )
    .unwrap();
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
    snapshot.tracks[0].instrument.as_mut().unwrap().state = state;
    let authored = |first: u64| {
        (0..notes)
            .map(|index| MidiNoteSnapshot {
                id: first + index as u64,
                start_raw: 0,
                duration_raw: duration,
                pitch: 36 + (index % 48) as u8,
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
    Project::from_snapshot(snapshot).unwrap()
}
fn edit(project: &Project, changes: &[(ParameterId, ParameterValue)]) -> Project {
    let device = project.tracks()[0].instrument().unwrap();
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
