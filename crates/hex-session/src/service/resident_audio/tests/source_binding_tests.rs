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
