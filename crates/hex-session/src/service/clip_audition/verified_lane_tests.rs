use super::*;
use crate::service::verified_source_tests::{Files, next, reservation, run};
use hex_project::{ClipGroupPosition, Command};

#[derive(Debug, PartialEq, Eq)]
enum Seen {
    Preview(ClipMovePreviewIdentity),
    Release(ClipMovePreviewIdentity),
    Cancel(ClipMovePreviewIdentity),
}
struct Sink {
    canonical: Arc<CanonicalAudioSourceSnapshot>,
    seen: Vec<Seen>,
}
impl WorkerAudioSink for Sink {
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        Ok(48_000)
    }
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
        self.canonical = source;
    }
    fn verified_clip_move_update(&mut self, update: VerifiedClipMoveUpdate) -> bool {
        let (source, event) = match update {
            VerifiedClipMoveUpdate::Preview(candidate) => (
                Arc::clone(candidate.canonical()),
                Seen::Preview(candidate.identity()),
            ),
            VerifiedClipMoveUpdate::Released {
                canonical,
                identity,
            } => (canonical, Seen::Release(identity)),
            VerifiedClipMoveUpdate::Cancelled {
                canonical,
                identity,
            } => (canonical, Seen::Cancel(identity)),
        };
        if !Arc::ptr_eq(&source, &self.canonical) {
            return false;
        }
        self.seen.push(event);
        true
    }
    fn poll(&mut self) {}
    fn close(&mut self) {}
}

#[test]
fn verified_lane_coalesces_and_delivers_exact_terminals_without_editing() {
    let files = Files::new();
    let service = Service::new(48_000, 16, 0).unwrap();
    next(&service);
    run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(reservation(2)),
        },
    );
    let imported = run(
        &service,
        Request::Import(files.wav("lane.wav", 48_000, 128, 8192)),
    );
    let event = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "destination".into(),
        }),
    );
    let source = Arc::clone(event.state.canonical_audio_source.as_ref().unwrap());
    let before = Arc::clone(&event.state.project);
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let track = before.tracks()[1].id();
    let target = ArrangementMoveAudition {
        context: source.context(),
        gesture: 1,
        sequence: 1,
        clips: vec![clip].into(),
        anchor: clip,
        destination: ArrangementMoveDestination::Existing(track),
        position: ClipGroupPosition::Audio(32),
        mode: ArrangementMoveMode::Move,
    };
    let mailbox = Mailbox::default();
    let mut lane = VerifiedLane::default();
    let mut sink = Sink {
        canonical: Arc::clone(&source),
        seen: Vec::new(),
    };
    let context = source.context();
    mailbox.set(Target::Move(target.clone()));
    let mut latest = target.clone();
    for sequence in 2..=1000 {
        latest.sequence = sequence;
        latest.position = ClipGroupPosition::Audio(48 + sequence);
        mailbox.set(Target::Move(latest.clone()));
    }
    assert!(!lane.drive(
        &mailbox,
        &event.state,
        &event.state.project,
        context,
        &mut sink,
        true
    ));
    assert_eq!(sink.seen, vec![Seen::Preview(latest.identity())]);
    assert!(!lane.drive(
        &mailbox,
        &event.state,
        &event.state.project,
        context,
        &mut sink,
        true
    ));
    let mut newer = latest.clone();
    newer.sequence = 1001;
    newer.position = ClipGroupPosition::Audio(64);
    mailbox.set(Target::Move(newer.clone()));
    lane.next_attempt = Some(Instant::now() + Duration::from_secs(1));
    assert!(lane.drive(
        &mailbox,
        &event.state,
        &event.state.project,
        context,
        &mut sink,
        true
    ));
    assert_eq!(sink.seen.len(), 1);
    lane.next_attempt = None;
    lane.drive(
        &mailbox,
        &event.state,
        &event.state.project,
        context,
        &mut sink,
        true,
    );
    assert_eq!(sink.seen[1], Seen::Preview(newer.identity()));
    mailbox.release(Kind::Move);
    lane.drive(
        &mailbox,
        &event.state,
        &event.state.project,
        context,
        &mut sink,
        true,
    );
    assert_eq!(sink.seen[2], Seen::Release(newer.identity()));
    lane.drive(
        &mailbox,
        &event.state,
        &event.state.project,
        context,
        &mut sink,
        true,
    );
    assert_eq!(sink.seen.len(), 3);
    mailbox.clear(Some(Kind::Move), false);
    lane.drive(
        &mailbox,
        &event.state,
        &event.state.project,
        context,
        &mut sink,
        true,
    );
    assert_eq!(sink.seen[3], Seen::Cancel(newer.identity()));
    assert_eq!(event.state.project.as_ref(), before.as_ref());
}

#[test]
fn verified_lane_rejects_stale_sources_and_unsupported_graphs() {
    let files = Files::new();
    let service = Service::new(48_000, 16, 0).unwrap();
    let initial = next(&service);
    let wrong_source = Arc::clone(initial.state.canonical_audio_source.as_ref().unwrap());
    run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(reservation(2)),
        },
    );
    let event = run(
        &service,
        Request::Import(files.wav("lane-stale.wav", 48_000, 128, 8192)),
    );
    let source = Arc::clone(event.state.canonical_audio_source.as_ref().unwrap());
    let clip = event.state.project.tracks()[0].clips()[0].id();
    let track = event.state.project.tracks()[0].id();
    let target = ArrangementMoveAudition {
        context: source.context(),
        gesture: 1,
        sequence: 1,
        clips: vec![clip].into(),
        anchor: clip,
        destination: ArrangementMoveDestination::Existing(track),
        position: ClipGroupPosition::Audio(32),
        mode: ArrangementMoveMode::Move,
    };
    let mut lane = VerifiedLane::default();
    let mailbox = Mailbox::default();
    let mut sink = Sink {
        canonical: Arc::clone(&source),
        seen: Vec::new(),
    };
    mailbox.set(Target::Move(target.clone()));
    let mut stale = source.context();
    stale.project_generation += 1;
    lane.drive(
        &mailbox,
        &event.state,
        &event.state.project,
        stale,
        &mut sink,
        true,
    );
    assert!(sink.seen.is_empty());
    let mut stale_revision = source.context();
    stale_revision.editor_revision += 1;
    lane.drive(
        &mailbox,
        &event.state,
        &event.state.project,
        stale_revision,
        &mut sink,
        true,
    );
    assert!(sink.seen.is_empty());
    sink.canonical = wrong_source;
    assert!(lane.drive(
        &mailbox,
        &event.state,
        &event.state.project,
        source.context(),
        &mut sink,
        true
    ));
    assert!(sink.seen.is_empty());
    sink.canonical = Arc::clone(&source);
    lane.next_attempt = None;
    lane.drive(
        &mailbox,
        &event.state,
        &event.state.project,
        source.context(),
        &mut sink,
        true,
    );
    assert_eq!(sink.seen, vec![Seen::Preview(target.identity())]);
    let replaced = run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(reservation(2)),
        },
    );
    lane.drive(
        &mailbox,
        &replaced.state,
        &replaced.state.project,
        replaced
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context(),
        &mut sink,
        true,
    );
    assert_eq!(sink.seen[1], Seen::Cancel(target.identity()));
    let added = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "midi".into(),
        }),
    );
    let track = added.state.project.tracks()[0].id();
    let midi = run(
        &service,
        Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        }),
    );
    let source = Arc::clone(midi.state.canonical_audio_source.as_ref().unwrap());
    sink.canonical = Arc::clone(&source);
    let clip = midi.state.project.tracks()[0].midi_clips()[0].id();
    let target = ArrangementMoveAudition {
        context: source.context(),
        gesture: 2,
        sequence: 1,
        clips: vec![clip].into(),
        anchor: clip,
        destination: ArrangementMoveDestination::Existing(track),
        position: ClipGroupPosition::Midi(hex_project::BeatTime::ONE),
        mode: ArrangementMoveMode::Move,
    };
    mailbox.set(Target::Move(target));
    lane.next_attempt = None;
    lane.drive(
        &mailbox,
        &midi.state,
        &midi.state.project,
        source.context(),
        &mut sink,
        true,
    );
    assert_eq!(
        sink.seen.len(),
        2,
        "unsupported MIDI graph is not sent to native control"
    );
}

#[test]
fn comparison_authoring_uses_the_current_project_and_exact_source() {
    use hex_engine::NativeDeviceClass;
    use hex_project::{DeviceComparisonAction, DeviceVariant};
    let files = Files::new();
    let service = Service::new(48_000, 16, 0).unwrap();
    next(&service);
    run(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(reservation(2)),
        },
    );
    let imported = run(
        &service,
        Request::Import(files.wav("comparison.wav", 48_000, 128, 8192)),
    );
    let track = imported.state.project.tracks()[0].id();
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let inserted = run(
        &service,
        Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
                "hex.utility",
            ))
            .unwrap(),
        }),
    );
    let device = inserted.state.project.tracks()[0].devices()[0].id();
    let source = Arc::clone(inserted.state.canonical_audio_source.as_ref().unwrap());
    let compare = |event: &crate::service::Event, expected| {
        run(
            &service,
            Request::CompareDevice {
                owner: crate::service::preset::Owner {
                    project_generation: source.context().project_generation,
                    revision: event.state.revision,
                    device,
                    class: NativeDeviceClass::Effect,
                    type_id: hex_engine::DeviceTypeId::from_static("hex.utility"),
                },
                expected,
                action: DeviceComparisonAction::Switch,
            },
        )
    };
    let first = compare(&inserted, DeviceVariant::A);
    let middle = compare(&first, DeviceVariant::B);
    let current = compare(&middle, DeviceVariant::A);
    assert_ne!(current.state.project, inserted.state.project);
    assert!(Arc::ptr_eq(
        current.state.canonical_audio_source.as_ref().unwrap(),
        &source
    ));
    assert_ne!(current.state.revision, source.context().editor_revision);
    let context = hex_engine::AudioSourceContext {
        project_generation: source.context().project_generation,
        editor_revision: current.state.revision,
    };
    let mut target = ArrangementMoveAudition {
        context,
        gesture: 1,
        sequence: 1,
        clips: vec![clip].into(),
        anchor: clip,
        destination: ArrangementMoveDestination::Existing(track),
        position: ClipGroupPosition::Audio(32),
        mode: ArrangementMoveMode::Move,
    };
    let mailbox = Mailbox::default();
    let mut lane = VerifiedLane::default();
    let mut sink = Sink {
        canonical: Arc::clone(&source),
        seen: Vec::new(),
    };
    target.context.editor_revision = middle.state.revision;
    mailbox.set(Target::Move(target.clone()));
    lane.drive(
        &mailbox,
        &current.state,
        &current.state.project,
        context,
        &mut sink,
        true,
    );
    assert!(
        sink.seen.is_empty(),
        "intermediate authoring revision must be refused"
    );
    target.context = context;
    mailbox.set(Target::Move(target.clone()));
    lane.drive(
        &mailbox,
        &current.state,
        &inserted.state.project,
        context,
        &mut sink,
        true,
    );
    assert!(sink.seen.is_empty(), "a mismatched project must be refused");
    lane.drive(
        &mailbox,
        &current.state,
        &current.state.project,
        context,
        &mut sink,
        true,
    );
    assert_eq!(sink.seen, vec![Seen::Preview(target.identity())]);
    mailbox.release(Kind::Move);
    lane.drive(
        &mailbox,
        &current.state,
        &current.state.project,
        context,
        &mut sink,
        true,
    );
    mailbox.clear(Some(Kind::Move), false);
    lane.drive(
        &mailbox,
        &current.state,
        &current.state.project,
        context,
        &mut sink,
        true,
    );
    assert_eq!(
        sink.seen,
        vec![
            Seen::Preview(target.identity()),
            Seen::Release(target.identity()),
            Seen::Cancel(target.identity())
        ]
    );
    assert_eq!(current.state.project.tracks()[0].clips()[0].start(), 0);
    assert!(Arc::ptr_eq(
        current.state.canonical_audio_source.as_ref().unwrap(),
        &source
    ));
}
