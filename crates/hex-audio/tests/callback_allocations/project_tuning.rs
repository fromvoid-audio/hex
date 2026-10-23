//! Real publication, held keyboard and retirement paths, with no audio device.
use super::*;

#[test]
fn builtin_fm4_and_operator_custom_tuning_handoff_and_release_allocate_nothing() {
    for kind in ["hex.instrument.fm4", "hex.instrument.operator"] {
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "tuned native".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let device = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            DeviceTypeId::new(kind).unwrap(),
        )
        .unwrap();
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let (mut control, mut callback) = prepared::channel(RATE).unwrap();
        let mut previous = None::<(Project, Arc<PreparedRevision>)>;
        let mut hint = None;
        for (index, hz) in [240.0, 333.0, 196.0, 333.0].into_iter().enumerate() {
            let mut scale = editor.project().tuning().scale();
            scale.set_reference(60, hz, 0).unwrap();
            editor
                .execute(Command::SetTuning(
                    hex_tuning::ProjectTuning::new(
                        true,
                        scale,
                        editor.project().tuning().audition(),
                    )
                    .unwrap(),
                ))
                .unwrap();
            let project = editor.project().clone();
            let revision = Arc::new(
                if let Some((source, old)) = &previous {
                    PreparedRevision::prepare_tuning_from(
                        &project,
                        &BTreeMap::new(),
                        Arc::new(SineTable::new()),
                        source,
                        old,
                    )
                } else {
                    PreparedRevision::prepare(
                        &project,
                        &BTreeMap::new(),
                        Arc::new(SineTable::new()),
                        None,
                    )
                }
                .unwrap(),
            );
            let mut candidate = Playback::from_prepared(Arc::clone(&revision));
            if let Some(hint) = &hint {
                candidate
                    .prepare_computer_keyboard_for_tuning(&project, track, Arc::clone(&input), hint)
                    .unwrap();
            } else {
                candidate
                    .prepare_computer_keyboard(&project, track, Arc::clone(&input))
                    .unwrap();
                hint = candidate.computer_keyboard_tuning_hint();
                assert!(hint.is_some());
            }
            if index == 2 {
                input.set_key(0, None).unwrap();
            }
            control.publish(candidate).unwrap();
            let mut output = [[0.0; 2]; 137];
            measured_block(&mut callback, &mut output);
            assert!(
                control.status.rejected_revision().is_none(),
                "{kind} at {index}: {:?}",
                control.status.rejected_revision()
            );
            assert!(!input.is_cancelled(), "{kind}");
            assert!(output.iter().flatten().all(|sample| sample.is_finite()));
            assert!(
                output.iter().flatten().any(|sample| sample.abs() > 0.00001),
                "{kind}: held and released voices must remain audible"
            );
            control.reclaim();
            previous = Some((project, revision));
        }
        control.request_shutdown();
        let mut output = [[0.0; 2]; 137];
        measured_shutdown(&mut callback, &mut output);
        control.reclaim();
    }
}

#[test]
fn held_keyboard_tuning_publication_and_retirement_allocate_nothing() {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "tuning".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let source_project = editor.project().clone();
    let first = Arc::new(
        PreparedRevision::prepare(
            &source_project,
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 100, 1))).unwrap();
    let mut playback = Playback::from_prepared(Arc::clone(&first));
    playback
        .prepare_computer_keyboard(&source_project, track, Arc::clone(&input))
        .unwrap();
    let hint = playback.computer_keyboard_tuning_hint().unwrap();
    let (mut control, mut callback) = prepared::channel(RATE).unwrap();
    control.publish(playback).unwrap();
    let mut output = [[0.0; 2]; 137];
    measured_block(&mut callback, &mut output);
    let mut old_project = source_project;
    let mut old_revision = first;
    let mut phase = (440.0 * 2.0_f64.powf((60.0 - 69.0) / 12.0)) * 137.0 / f64::from(RATE);
    for (index, hz) in [333.0, 196.0, 333.0].into_iter().enumerate() {
        let mut scale = old_project.tuning().scale();
        scale.set_reference(60, hz, 0).unwrap();
        editor
            .execute(Command::SetTuning(
                hex_tuning::ProjectTuning::new(true, scale, old_project.tuning().audition())
                    .unwrap(),
            ))
            .unwrap();
        let project = editor.project().clone();
        let revision = Arc::new(
            PreparedRevision::prepare_tuning_from(
                &project,
                &BTreeMap::new(),
                Arc::new(SineTable::new()),
                &old_project,
                &old_revision,
            )
            .unwrap(),
        );
        let mut next = Playback::from_prepared(Arc::clone(&revision));
        next.prepare_computer_keyboard_for_tuning(&project, track, Arc::clone(&input), &hint)
            .unwrap();
        control.publish(next).unwrap();
        measured_block(&mut callback, &mut output);
        assert!(control.status.rejected_revision().is_none());
        assert!(!input.is_cancelled());
        for (offset, frame) in output.iter().enumerate() {
            let expected = (std::f64::consts::TAU * (phase + hz * offset as f64 / f64::from(RATE)))
                .sin()
                * (100.0 / 127.0)
                * 0.1
                * (((index + 1) * 137 + offset) as f64 / (f64::from(RATE) * 0.005)).min(1.0);
            assert!((f64::from(frame[0]) - expected).abs() < 0.000002);
        }
        phase += hz * 137.0 / f64::from(RATE);
        control.reclaim();
        old_project = project;
        old_revision = revision;
    }
    input.set_key(0, None).unwrap();
    measured_block(&mut callback, &mut output);
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
}

#[test]
fn reference_audition_retargets_with_exact_phase_envelope_and_zero_callback_allocations() {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let input = Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(0));
    input.set_key(0, Some((60, 127, 1))).unwrap();
    let (mut control, mut callback) = prepared::channel(RATE).unwrap();
    let mut phase = 0.0_f64;
    let mut envelope = 0.0_f32;
    let coefficient = (-4.6 / (f64::from(RATE) * 0.01)).exp() as f32;
    let mut previous = None::<Arc<PreparedRevision>>;
    for hz in [240.0, 333.0, 196.0] {
        let mut scale = editor.project().tuning().scale();
        scale.set_reference(60, hz, 0).unwrap();
        editor
            .execute(Command::SetTuning(
                hex_tuning::ProjectTuning::new(true, scale, editor.project().tuning().audition())
                    .unwrap(),
            ))
            .unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::new(SineTable::new()),
                previous.as_ref().map(|r| r.session().as_ref()),
            )
            .unwrap(),
        );
        let mut candidate =
            Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
        candidate
            .prepare_tuning_audition(editor.project(), Arc::clone(&input))
            .unwrap();
        control.publish(candidate).unwrap();
        let mut output = [[0.0; 2]; 137];
        measured_block(&mut callback, &mut output);
        assert!(control.status.rejected_revision().is_none());
        assert!(!input.is_cancelled());
        assert_ne!(input.held_keys()[0] & (1 << 60), 0);
        for frame in output {
            envelope = 1.0 - (1.0 - envelope) * coefficient;
            let triangle = 1.0 - 4.0 * (phase - 0.5).abs();
            let expected = (triangle as f32 * envelope * 0.25 * 0.65).tanh();
            assert!((frame[0] - expected).abs() < 0.000002);
            assert_eq!(frame[0], frame[1]);
            phase = (phase + hz / f64::from(RATE)).fract();
        }
        control.reclaim();
        previous = Some(revision);
    }
    // Unsupported transient reference voices cannot veto the canonical graph.
    // Retuning back retries the still-held physical input without a new keydown.
    for (root_key, hz, unsupported) in [(48, 20_000.0, true), (60, 196.0, false)] {
        let mut scale = editor.project().tuning().scale();
        scale.set_reference(root_key, hz, 0).unwrap();
        editor
            .execute(Command::SetTuning(
                hex_tuning::ProjectTuning::new(true, scale, editor.project().tuning().audition())
                    .unwrap(),
            ))
            .unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::new(SineTable::new()),
                previous
                    .as_ref()
                    .map(|revision| revision.session().as_ref()),
            )
            .unwrap(),
        );
        let mut candidate =
            Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
        candidate
            .prepare_tuning_audition(editor.project(), Arc::clone(&input))
            .unwrap();
        control.publish(candidate).unwrap();
        let mut output = [[0.0; 2]; 137];
        measured_block(&mut callback, &mut output);
        assert!(control.status.rejected_revision().is_none());
        assert!(!input.is_cancelled());
        if unsupported {
            assert_eq!(input.held_keys(), [0; 2]);
            assert!(input.take_rejection());
            assert_eq!(output, [[0.0; 2]; 137]);
        } else {
            assert_ne!(input.held_keys()[0] & (1 << 60), 0);
            assert!(output.iter().any(|frame| frame[0] != 0.0));
            assert!(!input.take_rejection());
        }
        control.reclaim();
        previous = Some(revision);
    }
    control.command(PlaybackCommand::Stop).unwrap();
    let mut output = [[0.0; 2]; 137];
    measured_block(&mut callback, &mut output);
    assert!(input.is_cancelled());
    assert_eq!(input.held_keys(), [0; 2]);
    assert_eq!(output, [[0.0; 2]; 137]);
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
}
