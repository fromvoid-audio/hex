//! Retained sampler bindings remain reversible at the resident publication boundary.
use super::*;

fn context(revision: u64) -> hex_engine::AudioSourceContext {
    hex_engine::AudioSourceContext {
        project_generation: 0,
        editor_revision: revision,
    }
}

fn command(
    session: &mut Session,
    snapshot: &Snapshot,
    sink: &mut OfflineSink,
    command: Command,
    revision: u64,
) -> (Created, Snapshot) {
    session
        .command_resident_snapshot(
            command,
            context(revision),
            sink,
            sampler_export::Admission::default(),
            Some(snapshot),
        )
        .unwrap()
}

fn history(
    session: &mut Session,
    snapshot: &Snapshot,
    sink: &mut OfflineSink,
    redo: bool,
    revision: u64,
) -> Snapshot {
    session
        .history_resident_snapshot(
            redo,
            context(revision),
            sink,
            sampler_export::Admission::default(),
            Some(snapshot),
        )
        .unwrap()
        .expect("history transition")
}

fn prepare(session: &Session, sink: &mut OfflineSink) -> Snapshot {
    session
        .resident_seek_inputs(sampler_export::Admission::default(), None, None)
        .unwrap()
        .prepare(
            session.project(),
            sink,
            &session.media_bank(),
            session.sine_table(),
            &session.audio_transforms,
            &session.audio_clip_waveforms,
            context(0),
        )
        .unwrap()
}

#[test]
fn rebased_resident_publication_does_not_claim_the_old_preparation_origin() {
    let files = sampler_export::tests::Files::new(false);
    let session = interactive_sampler_session(&files);
    let observed = Arc::new(Observed::default());
    let mut sink = OfflineSink {
        observed: Arc::clone(&observed),
        pending: None,
    };
    let mut snapshot = session
        .publish_resident_snapshot(
            context(0),
            &mut sink,
            sampler_export::Admission::default(),
            None,
        )
        .unwrap();
    assert_eq!(
        observed
            .publications
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .0
            .source_origin,
        ResidentSourceOrigin::CanonicalCandidate
    );
    let inputs = session
        .resident_seek_inputs(sampler_export::Admission::default(), None, None)
        .unwrap();
    inputs
        .publish(&mut snapshot, session.project(), &mut sink, context(1))
        .unwrap();
    assert_eq!(
        observed
            .publications
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .0
            .source_origin,
        ResidentSourceOrigin::Unknown
    );
}

#[test]
fn submitted_snapshots_match_only_their_actual_applied_output_and_revision() {
    let files = sampler_export::tests::Files::new(false);
    let session = interactive_sampler_session(&files);
    let observed = Arc::new(Observed::default());
    let mut sink = OfflineSink {
        observed: Arc::clone(&observed),
        pending: None,
    };
    let offline = session
        .publish_resident_snapshot(
            context(0),
            &mut sink,
            sampler_export::Admission::default(),
            None,
        )
        .unwrap();
    assert!(offline.submission.is_none());
    *observed.next_submission.lock().unwrap() = Some((7, 11));
    let first = session
        .publish_resident_snapshot(
            context(0),
            &mut sink,
            sampler_export::Admission::default(),
            None,
        )
        .unwrap();
    *observed.next_submission.lock().unwrap() = Some((7, 12));
    let second = session
        .publish_resident_snapshot(
            context(0),
            &mut sink,
            sampler_export::Admission::default(),
            None,
        )
        .unwrap();
    let applied = |output_identity, audio_revision, source_context, publication_in_flight| {
        ResidentPublicationReadiness::Applied {
            submission: ResidentRuntimeLoop {
                output_identity,
                audio_revision,
                source_context,
                source_origin: ResidentSourceOrigin::CanonicalCandidate,
                section_loop: None,
            },
            publication_in_flight,
        }
    };
    let exact = applied(7, 11, Some(context(0)), false);
    let ResidentPublicationReadiness::Applied { submission, .. } = exact else {
        panic!("applied fixture")
    };
    for readiness in [
        ResidentPublicationReadiness::Pending {
            submission,
            applied_revision: 11,
            publication_in_flight: true,
        },
        ResidentPublicationReadiness::Refused {
            submission,
            applied_revision: 10,
            publication_in_flight: false,
        },
    ] {
        assert!(!first.submission.unwrap().matches_applied(readiness));
    }
    assert!(first.submission.unwrap().matches_applied(exact));
    assert!(!second.submission.unwrap().matches_applied(exact));
    assert!(
        !first
            .submission
            .unwrap()
            .matches_applied(applied(8, 11, Some(context(0)), false))
    );
    assert!(
        !first
            .submission
            .unwrap()
            .matches_applied(applied(7, 11, Some(context(1)), false))
    );
    assert!(
        !first
            .submission
            .unwrap()
            .matches_applied(applied(7, 11, Some(context(0)), true))
    );
    assert!(
        !first
            .submission
            .unwrap()
            .matches_applied(ResidentPublicationReadiness::NoOutput)
    );
    observed.refuse.store(true, Ordering::Release);
    let inputs = session
        .resident_seek_inputs(sampler_export::Admission::default(), None, None)
        .unwrap();
    let mut snapshot = first;
    assert!(
        inputs
            .publish(&mut snapshot, session.project(), &mut sink, context(0))
            .is_err()
    );
    assert!(snapshot.submission.unwrap().matches_applied(exact));
    observed.refuse.store(false, Ordering::Release);
    *observed.next_submission.lock().unwrap() = Some((7, 13));
    inputs
        .publish(&mut snapshot, session.project(), &mut sink, context(1))
        .unwrap();
    assert!(
        snapshot.submission.is_none(),
        "a rebased context cannot identify the original preparation"
    );
    *observed.next_submission.lock().unwrap() = Some((0, 14));
    inputs
        .publish(&mut snapshot, session.project(), &mut sink, context(0))
        .unwrap();
    assert!(
        snapshot.submission.is_none(),
        "zero output identity is not a submitted source"
    );
    *observed.next_submission.lock().unwrap() = Some((7, 15));
    inputs
        .publish(&mut snapshot, session.project(), &mut sink, context(0))
        .unwrap();
    assert_eq!(snapshot.submission.unwrap().audio_revision, 15);
    inputs
        .publish(&mut snapshot, session.project(), &mut sink, context(0))
        .unwrap();
    assert!(
        snapshot.submission.is_none(),
        "offline custody is not an installed source"
    );
}

fn render(snapshot: &Snapshot) -> Vec<hex_engine::Frame> {
    let mut playback = super::super::playback_with_loop(snapshot, None).unwrap();
    playback
        .try_apply_action(hex_engine::PlaybackAction::Transport(
            hex_engine::PlaybackCommand::Play,
        ))
        .unwrap();
    let mut frames = vec![[0.0; 2]; 12];
    playback.render(&mut frames).unwrap();
    frames
}

fn assert_binding(
    session: &Session,
    track: hex_project::TrackId,
    source_id: hex_project::SourceId,
    device_id: hex_project::DeviceId,
    local_effect: hex_project::DeviceId,
    bound: bool,
) {
    let project = session.project();
    let source = project.native_source(source_id);
    if bound {
        let source = source.expect("retained source");
        let track = project.track(track).unwrap();
        assert_eq!(track.source_id(), Some(source_id));
        assert_eq!(track.instrument().unwrap().id(), device_id);
        assert_eq!(source.instrument().id(), device_id);
    } else {
        assert!(project.track(track).unwrap().source_id().is_none());
        assert!(project.track(track).unwrap().instrument().is_none());
    }
    let track_view = project.track(track).unwrap();
    assert!(
        track_view
            .devices()
            .iter()
            .any(|device| device.id() == local_effect)
    );
    if let Some(source) = source {
        assert_eq!(
            source
                .devices()
                .map(|device| device.id())
                .collect::<Vec<_>>(),
            vec![device_id],
            "track-local effects must not enter the retained source"
        );
    }
}

#[test]
fn retained_sampler_unlink_delete_rebind_undo_redo_preserves_resident_pcm() {
    let files = sampler_export::tests::Files::new(false);
    let mut session = interactive_sampler_session(&files);
    let observed = Arc::new(Observed::default());
    let mut sink = OfflineSink {
        observed: Arc::clone(&observed),
        pending: None,
    };
    let mut snapshot = prepare(&session, &mut sink);
    let track = session.project().tracks()[0].id();
    let source_id = session.project().track(track).unwrap().source_id().unwrap();
    let device_id = session
        .project()
        .native_source(source_id)
        .unwrap()
        .instrument()
        .id();
    let source_state = session
        .project()
        .native_source(source_id)
        .unwrap()
        .instrument()
        .state()
        .clone();

    let (_, next) = command(
        &mut session,
        &snapshot,
        &mut sink,
        Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        },
        1,
    );
    snapshot = next;
    let Some(clip) = session.project().tracks()[0]
        .midi_clips()
        .first()
        .map(|clip| clip.id())
    else {
        panic!("MIDI clip")
    };
    let (_, next) = command(
        &mut session,
        &snapshot,
        &mut sink,
        Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        },
        2,
    );
    snapshot = next;

    let effect_state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    let (created, next) = command(
        &mut session,
        &snapshot,
        &mut sink,
        Command::InsertDevice {
            track,
            before: None,
            state: effect_state,
        },
        3,
    );
    let Created::Device(local_effect) = created else {
        panic!("track-local effect")
    };
    snapshot = next;
    let baseline = render(&snapshot);
    assert!(baseline.iter().flatten().any(|sample| *sample != 0.0));
    assert_binding(&session, track, source_id, device_id, local_effect, true);

    let project_before_refusal = session.project().clone();
    let can_undo_before_refusal = session.can_undo();
    observed.refuse.store(true, Ordering::Release);
    assert!(
        session
            .command_resident_snapshot(
                Command::BindTrackSource {
                    track,
                    source: None,
                },
                context(4),
                &mut sink,
                sampler_export::Admission::default(),
                Some(&snapshot),
            )
            .is_err()
    );
    observed.refuse.store(false, Ordering::Release);
    assert_eq!(session.project(), &project_before_refusal);
    assert_eq!(session.can_undo(), can_undo_before_refusal);
    assert_eq!(render(&snapshot), baseline);

    let (_, next) = command(
        &mut session,
        &snapshot,
        &mut sink,
        Command::BindTrackSource {
            track,
            source: None,
        },
        5,
    );
    snapshot = next;
    assert_binding(&session, track, source_id, device_id, local_effect, false);
    assert!(
        render(&snapshot)
            .iter()
            .flatten()
            .all(|sample| *sample == 0.0)
    );

    snapshot = history(&mut session, &snapshot, &mut sink, false, 6);
    assert_binding(&session, track, source_id, device_id, local_effect, true);
    assert_eq!(render(&snapshot), baseline);
    snapshot = history(&mut session, &snapshot, &mut sink, true, 7);
    assert_binding(&session, track, source_id, device_id, local_effect, false);

    let (_, next) = command(
        &mut session,
        &snapshot,
        &mut sink,
        Command::RemoveNativeSource(source_id),
        8,
    );
    snapshot = next;
    assert!(session.project().native_source(source_id).is_none());
    snapshot = history(&mut session, &snapshot, &mut sink, false, 9);
    assert_eq!(
        session
            .project()
            .native_source(source_id)
            .unwrap()
            .instrument()
            .state(),
        &source_state
    );
    assert_binding(&session, track, source_id, device_id, local_effect, false);
    snapshot = history(&mut session, &snapshot, &mut sink, true, 10);
    assert!(session.project().native_source(source_id).is_none());
    snapshot = history(&mut session, &snapshot, &mut sink, false, 11);

    let (_, next) = command(
        &mut session,
        &snapshot,
        &mut sink,
        Command::BindTrackSource {
            track,
            source: Some(source_id),
        },
        12,
    );
    snapshot = next;
    assert_binding(&session, track, source_id, device_id, local_effect, true);
    assert_eq!(render(&snapshot), baseline);
    snapshot = history(&mut session, &snapshot, &mut sink, false, 13);
    assert_binding(&session, track, source_id, device_id, local_effect, false);
    snapshot = history(&mut session, &snapshot, &mut sink, true, 14);
    assert_binding(&session, track, source_id, device_id, local_effect, true);
    assert_eq!(render(&snapshot), baseline);

    let (_, next) = command(
        &mut session,
        &snapshot,
        &mut sink,
        Command::RemoveTrack(track),
        15,
    );
    snapshot = next;
    assert!(session.project().track(track).is_none());
    let retained = session.project().native_source(source_id).unwrap();
    assert_eq!(retained.instrument().id(), device_id);
    assert_eq!(retained.instrument().state(), &source_state);
    snapshot = history(&mut session, &snapshot, &mut sink, false, 16);
    assert_binding(&session, track, source_id, device_id, local_effect, true);
    assert_eq!(render(&snapshot), baseline);
}

#[test]
fn source_effect_binding_commits_only_after_its_terminal_and_survives_history() {
    let observed = Arc::new(Observed::default());
    let mut sink = OfflineSink {
        observed: Arc::clone(&observed),
        pending: None,
    };
    let bytes = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 3;
    let mut session = Session::new(48_000, 8, 1024)
        .unwrap()
        .with_native_reservation(crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: bytes,
                max_construction_peak_bytes: bytes,
            },
            max_scan_frames: 1024,
        })
        .unwrap();
    let Created::Track(track) = session
        .command(Command::AddMidiTrack {
            name: "receiver".into(),
        })
        .unwrap()
    else {
        panic!("receiver")
    };
    let Created::NativeSource(source) = session
        .command(Command::CreateNativeSource {
            name: "shared".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source")
    };
    let parent = session
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    let Created::Device(child) = session
        .command(Command::InsertNestedDevice {
            parent,
            state: hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Effect,
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("child")
    };
    let before = session.project().snapshot();
    let original = prepare(&session, &mut sink);
    let bind = || Command::BindTrackSource {
        track,
        source: Some(source),
    };

    observed
        .refuse_source_confirmation
        .store(true, Ordering::Release);
    let rejected = session.command_resident_snapshot(
        bind(),
        context(1),
        &mut sink,
        sampler_export::Admission::default(),
        Some(&original),
    );
    assert!(
        matches!(rejected, Err(SessionError::NativeAudio(message)) if message == "fixture callback refused source edit")
    );
    assert_eq!(session.project().snapshot(), before);
    assert_eq!(observed.source_confirmations.load(Ordering::Acquire), 1);

    observed
        .refuse_source_confirmation
        .store(false, Ordering::Release);
    let (_, active) = command(&mut session, &original, &mut sink, bind(), 2);
    let bound = session.project().snapshot();
    assert_eq!(
        session.project().native_source(source).unwrap().receiver(),
        Some(track)
    );
    assert_eq!(
        session
            .project()
            .native_source(source)
            .unwrap()
            .post_instrument_effects()[0]
            .id(),
        child
    );
    let undone = history(&mut session, &active, &mut sink, false, 3);
    assert_eq!(session.project().snapshot(), before);
    let rebound = history(&mut session, &undone, &mut sink, true, 4);
    assert_eq!(session.project().snapshot(), bound);
    assert_eq!(observed.source_confirmations.load(Ordering::Acquire), 4);

    use hex_project::{
        DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, ModulationAmount,
        ModulationDestination, ModulationOutputId, ModulationRange, ModulationRouteId,
        ModulationSourceId, Modulator,
    };
    let next = session.project().next_identity();
    let modulator = ModulationSourceId::new(next).unwrap();
    let bank = DeviceModulation::new(
        vec![Modulator {
            id: modulator,
            color: Default::default(),
            enabled: true,
            state: hex_engine::lfo_modulator::Settings::default()
                .encode()
                .unwrap(),
        }],
        vec![DeviceModulationRoute {
            id: ModulationRouteId::new(next + 1).unwrap(),
            source: modulator,
            output: ModulationOutputId::MAIN,
            destination: ModulationDestination::Device {
                device_type: session.project().device(child).unwrap().state().type_id(),
                target: DeviceAutomationTarget::HostOutputGain,
                vst3_class: None,
            },
            amount: ModulationAmount::new(0.05).unwrap(),
            range: ModulationRange::default(),
            enabled: true,
        }],
    )
    .unwrap();
    let edit = || Command::ReplaceDeviceModulation {
        device: child,
        modulation: bank.clone(),
    };
    observed
        .refuse_source_confirmation
        .store(true, Ordering::Release);
    let rejected = session.command_resident_snapshot(
        edit(),
        context(5),
        &mut sink,
        sampler_export::Admission::default(),
        Some(&rebound),
    );
    assert!(matches!(rejected, Err(SessionError::NativeAudio(message))
        if message == "fixture callback refused source edit"));
    assert_eq!(session.project().snapshot(), bound);

    observed
        .refuse_source_confirmation
        .store(false, Ordering::Release);
    let (_, edited) = command(&mut session, &rebound, &mut sink, edit(), 6);
    assert_eq!(session.project().device(child).unwrap().modulation(), &bank);
    let _ = history(&mut session, &edited, &mut sink, false, 7);
    let mut expected = bound;
    expected.next_id = next + 2;
    assert_eq!(session.project().snapshot(), expected);
    assert_eq!(observed.source_confirmations.load(Ordering::Acquire), 7);
}

#[test]
fn unassigned_sampler_parameter_owner_validates_and_commits_resident_history() {
    let files = sampler_export::tests::Files::new(false);
    let mut session = interactive_sampler_session(&files);
    let mut sink = OfflineSink {
        observed: Arc::new(Observed::default()),
        pending: None,
    };
    let snapshot = prepare(&session, &mut sink);
    let track = session.project().tracks()[0].id();
    let source = session.project().track(track).unwrap().source_id().unwrap();
    let device = session
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    let (_, snapshot) = command(
        &mut session,
        &snapshot,
        &mut sink,
        Command::BindTrackSource {
            track,
            source: None,
        },
        1,
    );
    let (Created::NativeSource(other), snapshot) = command(
        &mut session,
        &snapshot,
        &mut sink,
        Command::CreateNativeSource {
            name: "other".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        },
        2,
    ) else {
        panic!("other source")
    };
    let original = session.project().clone();
    let other_original = original.native_source(other).unwrap().clone();
    let make_edit = |owner, generation, revision, cancel| SamplerParameterEdit {
        request_id: 1,
        project_generation: generation,
        revision,
        owner,
        device,
        parameter: hex_engine::SamplerParameter::Gain,
        value: 0.5,
        cancel,
    };
    let cancel = || Arc::new(AtomicBool::new(false));
    for mut invalid in [
        make_edit(SamplerParameterOwner::Source(source), 1, 2, cancel()),
        make_edit(SamplerParameterOwner::Source(source), 0, 1, cancel()),
        make_edit(SamplerParameterOwner::Track(track), 0, 2, cancel()),
        make_edit(
            SamplerParameterOwner::Source(source),
            0,
            2,
            Arc::new(AtomicBool::new(true)),
        ),
        make_edit(SamplerParameterOwner::Source(other), 0, 2, cancel()),
    ]
    .map(SamplerEdit::Parameter)
    {
        assert!(invalid.validate(&session, 0, 2).is_err());
        assert!(invalid.validate_resident(&session, 0, 2).is_err());
        assert_eq!(session.project(), &original);
    }
    let mut edit = SamplerEdit::Parameter(make_edit(
        SamplerParameterOwner::Source(source),
        0,
        2,
        cancel(),
    ));
    assert!(edit.validate(&session, 0, 2).unwrap());
    assert!(edit.validate_resident(&session, 0, 2).unwrap());
    let (_, edited_snapshot) = session
        .sampler_resident_edit(
            edit,
            context(3),
            &mut sink,
            sampler_export::Admission::default(),
            Some(&snapshot),
        )
        .unwrap()
        .expect("parameter change");
    let edited = session.project().clone();
    assert_eq!(edited.track(track).unwrap().source_id(), None);
    assert_eq!(
        edited.native_source(source).unwrap().instrument().id(),
        device
    );
    assert_eq!(
        hex_engine::sampler_parameter_value_on_control(
            hex_engine::SamplerParameter::Gain,
            edited.device(device).unwrap().state()
        )
        .unwrap(),
        0.5
    );
    assert_eq!(edited.native_source(other).unwrap(), &other_original);
    let undo = history(&mut session, &edited_snapshot, &mut sink, false, 4);
    assert_eq!(session.project(), &original);
    let redo = history(&mut session, &undo, &mut sink, true, 5);
    assert_eq!(session.project(), &edited);
    let (_, bound) = command(
        &mut session,
        &redo,
        &mut sink,
        Command::BindTrackSource {
            track,
            source: Some(source),
        },
        6,
    );
    let bound_project = session.project().clone();
    let mut stale = SamplerEdit::Parameter(make_edit(
        SamplerParameterOwner::Source(source),
        0,
        6,
        cancel(),
    ));
    assert!(stale.validate(&session, 0, 6).is_err());
    assert!(stale.validate_resident(&session, 0, 6).is_err());
    assert_eq!(session.project(), &bound_project);
    let mut track_edit = SamplerEdit::Parameter(make_edit(
        SamplerParameterOwner::Track(track),
        0,
        6,
        cancel(),
    ));
    assert!(!track_edit.validate(&session, 0, 6).unwrap());
    assert!(!track_edit.validate_resident(&session, 0, 6).unwrap());
    let replacement = hex_project::default_instrument_state().unwrap();
    let (_, replaced) = command(
        &mut session,
        &bound,
        &mut sink,
        Command::ReplaceTrackInstrument {
            track,
            replacing: device,
            state: replacement,
        },
        7,
    );
    let (_, unbound) = command(
        &mut session,
        &replaced,
        &mut sink,
        Command::BindTrackSource {
            track,
            source: None,
        },
        8,
    );
    let after_replace = session.project().clone();
    let mut stale = SamplerEdit::Parameter(make_edit(
        SamplerParameterOwner::Source(source),
        0,
        8,
        cancel(),
    ));
    assert!(stale.validate(&session, 0, 8).is_err());
    assert!(stale.validate_resident(&session, 0, 8).is_err());
    assert_eq!(session.project(), &after_replace);
    let (_, removed) = command(
        &mut session,
        &unbound,
        &mut sink,
        Command::RemoveNativeSource(source),
        9,
    );
    let after_remove = session.project().clone();
    let mut stale = SamplerEdit::Parameter(make_edit(
        SamplerParameterOwner::Source(source),
        0,
        9,
        cancel(),
    ));
    assert!(stale.validate(&session, 0, 9).is_err());
    assert!(stale.validate_resident(&session, 0, 9).is_err());
    assert_eq!(session.project(), &after_remove);
    assert_eq!(
        session.project().native_source(other).unwrap(),
        &other_original
    );
    drop(removed);
}

#[test]
fn resident_worker_edits_unassigned_sampler_without_binding_a_track() {
    let observed = Arc::new(Observed::default());
    let bytes = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        1024,
        None,
        Box::new(OfflineSink {
            observed,
            pending: None,
        }),
        crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: bytes * 2,
                max_construction_peak_bytes: bytes * 3,
            },
            max_scan_frames: 1024,
        },
    )
    .unwrap();
    let receive = || {
        service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap()
    };
    receive();
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "unassigned".into(),
        }))
        .unwrap();
    let added = receive();
    assert!(added.error.is_none(), "{:?}", added.error);
    let track = added.state.project.tracks()[0].id();
    let files = sampler_export::tests::Files::new(false);
    service
        .submit(Request::LoadFirstSampler {
            request_id: 2,
            project_generation: 0,
            revision: added.state.revision,
            track,
            slot: SamplerLoadSlot::New(hex_project::InstrumentLoadPlacement::First),
            path: files.source().to_owned(),
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let loaded = receive();
    assert!(loaded.error.is_none(), "{:?}", loaded.error);
    let source = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .source_id()
        .unwrap();
    let device = loaded
        .state
        .project
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track,
            source: None,
        }))
        .unwrap();
    let created = receive();
    assert!(created.error.is_none(), "{:?}", created.error);
    service
        .submit(Request::SamplerEdit(SamplerEdit::Parameter(
            SamplerParameterEdit {
                request_id: 1,
                project_generation: 0,
                revision: created.state.revision,
                owner: SamplerParameterOwner::Source(source),
                device,
                parameter: hex_engine::SamplerParameter::Gain,
                value: 0.5,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        )))
        .unwrap();
    let edited = receive();
    assert!(edited.error.is_none(), "{:?}", edited.error);
    assert_eq!(edited.sampler_edit_complete, Some(1));
    assert_eq!(edited.state.project.track(track).unwrap().source_id(), None);
    assert_eq!(
        hex_engine::sampler_parameter_value_on_control(
            hex_engine::SamplerParameter::Gain,
            edited
                .state
                .project
                .native_source(source)
                .unwrap()
                .instrument()
                .state(),
        )
        .unwrap(),
        0.5
    );
    service.submit(Request::Undo).unwrap();
    let undone = receive();
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.project, created.state.project);
    service.submit(Request::Redo).unwrap();
    let redone = receive();
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(redone.state.project, edited.state.project);
}

#[test]
fn resident_worker_loads_and_replaces_unassigned_sampler_originals() {
    let observed = Arc::new(Observed::default());
    let bytes = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        1024,
        None,
        Box::new(OfflineSink {
            observed,
            pending: None,
        }),
        crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: bytes * 2,
                max_construction_peak_bytes: bytes * 3,
            },
            max_scan_frames: 1024,
        },
    )
    .unwrap();
    let receive = || {
        service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap()
    };
    receive();
    let empty = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sampler"),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::CreateNativeSource {
            name: "unassigned".into(),
            devices: vec![empty],
            instrument_index: 0,
        }))
        .unwrap();
    let created = receive();
    assert!(created.error.is_none(), "{:?}", created.error);
    let Some(Created::NativeSource(source)) = created.created else {
        panic!("source")
    };
    let device = created
        .state
        .project
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    service
        .submit(Request::Edit(Command::CreateNativeSource {
            name: "other".into(),
            devices: vec![
                created
                    .state
                    .project
                    .device(device)
                    .unwrap()
                    .state()
                    .clone(),
            ],
            instrument_index: 0,
        }))
        .unwrap();
    let other_event = receive();
    let Some(Created::NativeSource(other)) = other_event.created else {
        panic!("other source")
    };
    let other_device = other_event
        .state
        .project
        .native_source(other)
        .unwrap()
        .instrument()
        .id();
    service
        .submit(Request::Edit(Command::RemoveNativeSource(other)))
        .unwrap();
    let created = receive();
    assert!(created.error.is_none(), "{:?}", created.error);
    let files = sampler_export::tests::Files::new(false);
    let request = |state: &ViewState, id, cancel| {
        Request::SamplerEdit(SamplerEdit::Replacement(SamplerReplacementEdit {
            request_id: id,
            project_generation: 0,
            revision: state.revision,
            owner: hex_project::InstrumentAssetOwner::Source(source),
            device,
            expected_state: state.project.device(device).unwrap().state().clone(),
            path: files.source().to_owned(),
            cancel: Arc::new(AtomicBool::new(cancel)),
        }))
    };
    service.submit(request(&created.state, 1, true)).unwrap();
    let cancelled = receive();
    assert!(cancelled.error.is_some());
    assert_eq!(cancelled.sampler_edit_complete, Some(1));
    assert_eq!(cancelled.state.project, created.state.project);
    for case in 0..5 {
        let mut bad = request(&created.state, 10 + case, false);
        if let Request::SamplerEdit(SamplerEdit::Replacement(edit)) = &mut bad {
            match case {
                0 => edit.revision = edit.revision.saturating_add(1),
                1 => {
                    edit.expected_state = edit
                        .expected_state
                        .clone()
                        .with_input_gain_millidb(1000)
                        .unwrap()
                }
                2 => edit.path = files.source().with_extension("missing"),
                3 => edit.device = other_device,
                _ => edit.owner = hex_project::InstrumentAssetOwner::Source(other),
            }
        }
        service.submit(bad).unwrap();
        let refused = receive();
        assert!(refused.error.is_some(), "case {case}");
        assert_eq!(refused.sampler_edit_complete, Some(10 + case));
        assert_eq!(refused.state.project, created.state.project);
    }
    service.submit(request(&created.state, 2, false)).unwrap();
    let first = receive();
    assert!(first.error.is_none(), "{:?}", first.error);
    assert_eq!(first.sampler_edit_complete, Some(2));
    assert_eq!(first.state.project.source_contributors(source).count(), 0);
    assert_eq!(
        first
            .state
            .project
            .native_source(source)
            .unwrap()
            .instrument()
            .id(),
        device
    );
    assert_eq!(first.state.project.assets().count(), 1);
    service.submit(request(&first.state, 3, false)).unwrap();
    let second = receive();
    assert!(second.error.is_none(), "{:?}", second.error);
    assert_eq!(second.sampler_edit_complete, Some(3));
    assert_eq!(second.state.project.assets().count(), 2);
    service.submit(Request::Undo).unwrap();
    let undone = receive();
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(
        undone.state.project.device(device).unwrap().state(),
        first.state.project.device(device).unwrap().state()
    );
    service.submit(Request::Undo).unwrap();
    let empty_again = receive();
    assert!(empty_again.error.is_none(), "{:?}", empty_again.error);
    assert!(hex_engine::is_empty_sampler_state_on_control(
        empty_again.state.project.device(device).unwrap().state()
    ));
    service.submit(Request::Redo).unwrap();
    let first_again = receive();
    assert!(first_again.error.is_none(), "{:?}", first_again.error);
    assert_eq!(
        first_again.state.project.device(device).unwrap().state(),
        first.state.project.device(device).unwrap().state()
    );
    service.submit(Request::Redo).unwrap();
    let redone = receive();
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(
        redone.state.project.device(device).unwrap().state(),
        second.state.project.device(device).unwrap().state()
    );
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "assigned".into(),
        }))
        .unwrap();
    let track_event = receive();
    let Some(Created::Track(track)) = track_event.created else {
        panic!("track")
    };
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track,
            source: Some(source),
        }))
        .unwrap();
    let assigned = receive();
    assert!(assigned.error.is_none(), "{:?}", assigned.error);
    service.submit(request(&assigned.state, 20, false)).unwrap();
    let refused = receive();
    assert!(refused.error.is_some());
    assert_eq!(refused.sampler_edit_complete, Some(20));
    assert_eq!(refused.state.project, assigned.state.project);
}

#[test]
fn preview_runtime_publication_updates_only_the_audible_snapshot() {
    use crate::service::resident_preview::{Identity, Preview};
    let files = sampler_export::tests::Files::new(false);
    let session = interactive_sampler_session(&files);
    let observed = Arc::new(Observed::default());
    let mut sink = OfflineSink {
        observed: Arc::clone(&observed),
        pending: None,
    };
    *observed.next_submission.lock().unwrap() = Some((7, 11));
    let canonical = session
        .publish_resident_snapshot(
            context(0),
            &mut sink,
            sampler_export::Admission::default(),
            None,
        )
        .unwrap();
    let original = canonical.submission.unwrap();
    let overlay = prepare(&session, &mut sink);
    let identity = Identity::Tuning(crate::service::TuningPreviewIdentity {
        project_generation: 0,
        canonical_revision: 0,
        sequence: 1,
        key: crate::tuning_gesture::TuningGestureKey {
            generation: 0,
            token: 1,
        },
    });
    let mut current = Some(canonical);
    let mut preview = Some(Preview {
        project: Arc::new(session.project().clone()),
        snapshot: overlay,
        identity,
    });
    let metadata = canonical_metadata(current.as_ref().unwrap(), context(0));
    *observed.next_submission.lock().unwrap() = Some((7, 12));
    crate::service::resident_preview::publish_inputs(
        &session,
        &mut current,
        &mut preview,
        &mut sink,
        sampler_export::Admission::default(),
        metadata,
        ResidentPublicationPolicy::InheritRuntime,
        RuntimeInputs::default(),
        None,
    )
    .unwrap();
    assert_eq!(current.as_ref().unwrap().submission, Some(original));
    assert_eq!(
        preview
            .as_ref()
            .unwrap()
            .snapshot
            .submission
            .unwrap()
            .audio_revision,
        12
    );
    observed.refuse.store(true, Ordering::Release);
    assert!(
        crate::service::resident_preview::publish_inputs(
            &session,
            &mut current,
            &mut preview,
            &mut sink,
            sampler_export::Admission::default(),
            metadata,
            ResidentPublicationPolicy::InheritRuntime,
            RuntimeInputs::default(),
            None,
        )
        .is_err()
    );
    assert_eq!(current.as_ref().unwrap().submission, Some(original));
    assert_eq!(
        preview
            .as_ref()
            .unwrap()
            .snapshot
            .submission
            .unwrap()
            .audio_revision,
        12
    );
    observed.refuse.store(false, Ordering::Release);
    *observed.next_submission.lock().unwrap() = Some((7, 13));
    session
        .publish_resident_runtime_inputs(
            current.as_mut().unwrap(),
            &mut sink,
            sampler_export::Admission::default(),
            metadata,
            ResidentPublicationPolicy::InheritRuntime,
            RuntimeInputs::default(),
            None,
        )
        .unwrap();
    assert_eq!(
        current.as_ref().unwrap().submission.unwrap().audio_revision,
        13
    );
}

#[test]
fn seek_publication_never_attributes_a_different_prepared_graph_to_current() {
    let files = sampler_export::tests::Files::new(false);
    let session = interactive_sampler_session(&files);
    let observed = Arc::new(Observed::default());
    let mut sink = OfflineSink {
        observed: Arc::clone(&observed),
        pending: None,
    };
    *observed.next_submission.lock().unwrap() = Some((7, 11));
    let mut current = session
        .publish_resident_snapshot(
            context(0),
            &mut sink,
            sampler_export::Admission::default(),
            None,
        )
        .unwrap();
    let different = prepare(&session, &mut sink);
    assert!(!Arc::ptr_eq(&current.prepared, &different.prepared));
    let playback = playback_with_loop(&different, None).unwrap();
    *observed.next_submission.lock().unwrap() = Some((7, 12));
    session
        .publish_resident_seek(
            &different,
            session.project(),
            &mut current,
            playback,
            &mut sink,
            sampler_export::Admission::default(),
            canonical_metadata(&different, context(0)),
            ResidentPublicationPolicy::InheritRuntime,
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(current.submission.is_none());
    let same = Snapshot {
        source_origin: current.source_origin,
        prepared_context: current.prepared_context,
        submission: None,
        prepared: Arc::clone(&current.prepared),
        resources: current.resources.capture_on_control(usize::MAX).unwrap(),
        inputs: current.inputs.clone(),
        file_sources: current.file_sources.clone(),
    };
    let playback = playback_with_loop(&same, None).unwrap();
    *observed.next_submission.lock().unwrap() = Some((7, 13));
    session
        .publish_resident_seek(
            &same,
            session.project(),
            &mut current,
            playback,
            &mut sink,
            sampler_export::Admission::default(),
            canonical_metadata(&same, context(0)),
            ResidentPublicationPolicy::InheritRuntime,
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(current.submission.unwrap().audio_revision, 13);
}
