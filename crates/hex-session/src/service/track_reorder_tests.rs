//! Captured row order uses ordinary prepare-before-commit and existing cached media.
use super::service_tests::{TestDir, next_event, wav_mono};
use super::*;
use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
use hex_engine::NativeDeviceClass;
use hex_lattice_contract::{DeviceTypeId, ParameterValue};
use hex_project::{BeatTime, TrackId};
use std::fs;

fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = next_event(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}
fn reorder(state: &ViewState, track: TrackId, before: Option<TrackId>) -> Request {
    Request::ReorderTracks {
        project_generation: 0,
        revision: state.revision,
        tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
        before,
    }
}
fn order(project: &Project) -> Vec<TrackId> {
    project.tracks().iter().map(|track| track.id()).collect()
}
fn prepared(event: &Event) -> Arc<hex_engine::PreparedRevision> {
    Arc::clone(
        event
            .playback
            .as_ref()
            .unwrap()
            .prepared_revision()
            .unwrap(),
    )
}

fn resident_pcm(event: &Event) -> hex_engine::Audio {
    let source = event.state.canonical_audio_source.as_ref().unwrap();
    let (_, crate::AudioSourceBinding::Resident(audio)) = source.required_sources().next().unwrap()
    else {
        panic!("resident source")
    };
    audio.clone()
}

#[test]
fn reorder_track_worker_preserves_cached_pcm_devices_and_one_history_edge() {
    let files = TestDir::new();
    let path = files.0.join("cached.wav");
    fs::write(&path, wav_mono(&[4096; 512])).unwrap();
    let service = Service::new(48_000, 32, 8192).unwrap();
    next_event(&service);
    let imported = run(&service, Request::Import(path.clone()));
    let audio_track = imported.state.project.tracks()[0].id();
    let asset = imported.state.project.assets().next().unwrap().id();
    let midi = run(
        &service,
        Request::InsertBrowserInstrumentTrack {
            project_generation: 0,
            revision: imported.state.revision,
            class: NativeDeviceClass::Instrument,
            type_id: DeviceTypeId::from_static("hex.instrument.sine"),
        },
    );
    let midi_track = midi.state.project.tracks()[1].id();
    let placed = run(
        &service,
        Request::Edit(Command::PlaceMidiClip {
            track: midi_track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        }),
    );
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    let original = run(
        &service,
        Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 80,
        }),
    );
    let original_prepared = prepared(&original);
    let original_pcm = resident_pcm(&original);
    let original_device = original
        .state
        .project
        .track(midi_track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    // The accepted Session owns decoded PCM. Reordering must not reopen media.
    fs::remove_file(path).unwrap();
    let moved = run(
        &service,
        reorder(&original.state, midi_track, Some(audio_track)),
    );
    assert_eq!(order(&moved.state.project), [midi_track, audio_track]);
    assert_eq!(moved.state.revision, original.state.revision + 1);
    for track in original.state.project.tracks() {
        assert_eq!(moved.state.project.track(track.id()), Some(track));
    }
    assert_eq!(
        moved.state.project.device(original_device),
        original.state.project.device(original_device)
    );
    assert!(Arc::ptr_eq(
        &moved.state.waveforms[&asset],
        &original.state.waveforms[&asset]
    ));
    let moved_prepared = prepared(&moved);
    assert!(original_pcm.shares_pcm_with(&resident_pcm(&moved)));
    let mut before = Playback::from_prepared(original_prepared.clone());
    let mut after = Playback::from_prepared(moved_prepared);
    before.apply(hex_engine::PlaybackCommand::PlayFrom(0));
    after.apply(hex_engine::PlaybackCommand::PlayFrom(0));
    let mut expected = [[0.0; 2]; 256];
    let mut actual = [[0.0; 2]; 256];
    before.render(&mut expected).unwrap();
    after.render(&mut actual).unwrap();
    assert!(expected.iter().flatten().any(|value| *value != 0.0));
    assert_eq!(actual, expected, "row order retains this audio+MIDI signal");
    let undone = run(&service, Request::Undo);
    assert_eq!(undone.state.project, original.state.project);
    assert!(undone.state.can_redo);
    // No-op moves preserve the existing redo edge and canonical source identity.
    for (track, before) in [
        (audio_track, Some(audio_track)),
        (audio_track, Some(midi_track)),
        (midi_track, None),
    ] {
        let noop = run(&service, reorder(&undone.state, track, before));
        assert_eq!(noop.state.revision, undone.state.revision);
        assert_eq!(noop.state.project, undone.state.project);
        assert!(noop.state.can_redo);
        assert!(noop.playback.is_none());
        assert!(Arc::ptr_eq(
            noop.state.canonical_audio_source.as_ref().unwrap(),
            undone.state.canonical_audio_source.as_ref().unwrap()
        ));
    }
    let redone = run(&service, Request::Redo);
    assert_eq!(redone.state.project, moved.state.project);

    let third = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "empty".into(),
        }),
    );
    let empty = third.state.project.tracks()[2].id();
    let grouped = run(
        &service,
        Request::ReorderTracks {
            project_generation: 0,
            revision: third.state.revision,
            // Input order differs from the current canonical MIDI/audio order.
            tracks: hex_project::TrackSelectionIds::new(&[audio_track, midi_track]).unwrap(),
            before: None,
        },
    );
    assert_eq!(
        order(&grouped.state.project),
        [empty, midi_track, audio_track]
    );
    assert_eq!(grouped.state.revision, third.state.revision + 1);
    for track in third.state.project.tracks() {
        assert_eq!(grouped.state.project.track(track.id()), Some(track));
    }
    assert!(original_pcm.shares_pcm_with(&resident_pcm(&grouped)));
    assert!(Arc::ptr_eq(
        &grouped.state.waveforms[&asset],
        &third.state.waveforms[&asset]
    ));
    let mut playback = Playback::from_prepared(prepared(&grouped));
    playback.apply(hex_engine::PlaybackCommand::PlayFrom(0));
    playback.render(&mut actual).unwrap();
    assert_eq!(
        actual, expected,
        "the group path retains cached audio and MIDI signal"
    );
    let undo = run(&service, Request::Undo);
    assert_eq!(undo.state.project, third.state.project);
    let noop = run(
        &service,
        Request::ReorderTracks {
            project_generation: 0,
            revision: undo.state.revision,
            tracks: hex_project::TrackSelectionIds::new(&[audio_track, midi_track]).unwrap(),
            before: Some(audio_track),
        },
    );
    assert_eq!(noop.state.revision, undo.state.revision);
    assert!(noop.playback.is_none() && noop.state.can_redo);
    assert_eq!(
        run(&service, Request::Redo).state.project,
        grouped.state.project
    );
}

#[test]
fn reorder_track_worker_refuses_stale_targets_and_keeps_active_gesture_uncommitted() {
    let service = Service::new(48_000, 32, 0).unwrap();
    next_event(&service);
    let first = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "first".into(),
        }),
    );
    let first_id = first.state.project.tracks()[0].id();
    let second = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "second".into(),
        }),
    );
    let second_id = second.state.project.tracks()[1].id();
    let deleted = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "removed".into(),
        }),
    );
    let missing = deleted.state.project.tracks()[2].id();
    let stable = run(&service, Request::Edit(Command::RemoveTrack(missing))).state;
    for (generation, revision, members, before) in [
        (1, stable.revision, vec![second_id], Some(first_id)),
        (0, stable.revision + 1, vec![second_id], Some(first_id)),
        (0, stable.revision, vec![missing], Some(first_id)),
        (0, stable.revision, vec![second_id], Some(missing)),
        (0, stable.revision, vec![first_id, missing], None),
    ] {
        service
            .submit(Request::ReorderTracks {
                project_generation: generation,
                revision,
                tracks: hex_project::TrackSelectionIds::new(&members).unwrap(),
                before,
            })
            .unwrap();
        let refused = next_event(&service);
        assert!(refused.error.is_some());
        assert!(
            refused.request_complete && refused.created.is_none() && refused.playback.is_none()
        );
        assert_eq!(refused.state.project, stable.project);
        assert_eq!(refused.state.revision, stable.revision);
        assert_eq!(
            (refused.state.can_undo, refused.state.can_redo),
            (stable.can_undo, stable.can_redo)
        );
        assert!(Arc::ptr_eq(
            refused.state.canonical_audio_source.as_ref().unwrap(),
            stable.canonical_audio_source.as_ref().unwrap()
        ));
    }
    let key = GestureKey {
        generation: 0,
        token: 71,
    };
    let begun = run(
        &service,
        Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track: first_id },
        }),
    );
    assert!(begun.gesture_ack.unwrap().accepted);
    let draft = run(
        &service,
        Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(0.25),
        }),
    );
    assert!(draft.gesture_ack.unwrap().accepted);
    service
        .submit(reorder(&stable, second_id, Some(first_id)))
        .unwrap();
    let refused = next_event(&service);
    assert!(refused.error.as_ref().unwrap().contains("active gesture"));
    assert!(refused.gesture_ack.is_none());
    assert!(refused.active_device_gesture.is_some());
    assert_eq!(refused.state.project, stable.project);
    assert_eq!(refused.state.revision, stable.revision);
    assert!(refused.playback.is_none());
    let tracks = hex_project::TrackSelectionIds::new(&[first_id, second_id]).unwrap();
    for action in [
        TrackClipboardAction::Copy(tracks.clone()),
        TrackClipboardAction::Cut(tracks.clone()),
        TrackClipboardAction::Duplicate {
            tracks,
            without_clips: true,
        },
        TrackClipboardAction::Paste { before: None },
    ] {
        service
            .submit(Request::TrackClipboard {
                project_generation: 0,
                revision: stable.revision,
                action,
            })
            .unwrap();
        let refused = next_event(&service);
        assert!(refused.error.as_deref().unwrap().contains("active gesture"));
        assert!(refused.active_device_gesture.is_some());
        assert_eq!(refused.state.project, stable.project);
        assert_eq!(refused.state.revision, stable.revision);
        assert!(refused.playback.is_none());
    }
    let canceled = run(
        &service,
        Request::DeviceGesture(GestureRequest::Cancel { key }),
    );
    assert!(canceled.gesture_ack.unwrap().accepted);
    assert_eq!(canceled.state.project, stable.project);
}

#[test]
fn inserted_default_track_initial_gain_is_prepared_and_undoes_atomically() {
    let service = Service::new(48_000, 16, 32).unwrap();
    next_event(&service);
    let before = run(
        &service,
        Request::Edit(Command::InsertDefaultTrack { before_track: None }),
    );
    let anchor = before.state.project.tracks()[0].id();
    let inserted = run(
        &service,
        Request::Edit(Command::InsertDefaultTrackWithGain {
            before_track: Some(anchor),
            gain: 0.25,
        }),
    );
    let Some(Created::Track(track)) = inserted.created else {
        panic!("track")
    };
    assert_eq!(inserted.state.project.tracks()[0].id(), track);
    assert_eq!(inserted.state.project.track(track).unwrap().gain(), 0.25);
    assert_eq!(inserted.state.project.track(anchor).unwrap().gain(), 1.0);
    assert!(
        inserted.playback.is_some(),
        "the gained track is included in prepared playback"
    );
    let after = inserted.state.project.snapshot();
    let undo = run(&service, Request::Undo);
    let mut expected = before.state.project.snapshot();
    expected.next_id = undo.state.project.snapshot().next_id;
    assert_eq!(undo.state.project.snapshot(), expected);
    let redo = run(&service, Request::Redo);
    assert_eq!(redo.state.project.snapshot(), after);
}
