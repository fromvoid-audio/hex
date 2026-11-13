//! Row order keeps the existing native publication and cached-source owners.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_engine::{DeviceTypeId, NativeDeviceClass};
use hex_project::{BeatTime, Command, TrackId};

fn reorder(event: &Event, track: TrackId, before: Option<TrackId>) -> Request {
    Request::ReorderTracks {
        project_generation: 0,
        revision: event.state.revision,
        tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
        before,
    }
}
fn order(event: &Event) -> Vec<TrackId> {
    event
        .state
        .project
        .tracks()
        .iter()
        .map(|t| t.id())
        .collect()
}
fn run_applied(h: &mut ResidentHarness, applied: &mut u64, request: Request) -> Event {
    let event = h.run(request);
    let (revision, metadata) = h.applied(*applied);
    *applied = revision;
    assert_eq!(metadata.context.editor_revision, event.state.revision);
    event
}
fn add_instrument(h: &mut ResidentHarness, applied: &mut u64, prior: &Event) -> Event {
    run_applied(
        h,
        applied,
        Request::InsertBrowserInstrumentTrack {
            new_track_gain: None,
            project_generation: 0,
            revision: prior.state.revision,
            class: NativeDeviceClass::Instrument,
            type_id: DeviceTypeId::from_static("hex.instrument.sine"),
        },
    )
}
fn resident_pcm(event: &Event) -> hex_engine::Audio {
    let source = event.state.canonical_audio_source.as_ref().unwrap();
    let (_, hex_session::AudioSourceBinding::Resident(audio)) =
        source.required_sources().next().unwrap()
    else {
        panic!("resident source")
    };
    audio.clone()
}

#[test]
fn reorder_track_native_publication_preserves_runtime_and_refusal_is_atomic() {
    let _native = native_fixture();
    native_reorder_case(false);
    native_reorder_case(true);
}

fn native_reorder_case(group: bool) {
    let files = Files::new();
    let path = files.wav(48_000, 8192);
    let mut h = ResidentHarness::with_frame_limit(32_768);
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = run_applied(&mut h, &mut applied, Request::Import(path.clone()));
    let audio = imported.state.project.tracks()[0].id();
    let instrument = add_instrument(&mut h, &mut applied, &imported);
    let midi = instrument.state.project.tracks()[1].id();
    let placed = run_applied(
        &mut h,
        &mut applied,
        Request::Edit(Command::PlaceMidiClip {
            track: midi,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        }),
    );
    let Some(hex_project::Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    run_applied(
        &mut h,
        &mut applied,
        Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 80,
        }),
    );
    let third = run_applied(
        &mut h,
        &mut applied,
        Request::Edit(Command::AddTrack {
            name: "empty".into(),
        }),
    );
    let empty = third.state.project.tracks()[2].id();
    let original = run_applied(
        &mut h,
        &mut applied,
        Request::Edit(Command::SetAudioFeed {
            source: hex_project::RoutingOwner::Track(audio),
            destination: midi,
            gain: 0.5,
        }),
    );
    let pcm = resident_pcm(&original);
    let asset = original.state.project.assets().next().unwrap().id();
    let reference = hex_engine::PreparedRevision::prepare(
        &original.state.project,
        &std::collections::BTreeMap::from([(asset, pcm.clone())]),
        Arc::new(hex_engine::SineTable::new()),
        None,
    )
    .unwrap();
    let mut uninterrupted =
        hex_engine::playback::Playback::try_from_prepared_with_session(Arc::new(reference))
            .unwrap();
    uninterrupted.apply(PlaybackCommand::PlayFrom(0));
    let mut expected = [[0.0; 2]; 64];
    let output = h.audio.as_ref().unwrap().control.output_identity();
    fs::remove_file(path).unwrap();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    uninterrupted.render(&mut expected).unwrap();
    assert_eq!(
        h.render(),
        expected,
        "uninterrupted oracle starts at the same cursor"
    );
    assert!(
        expected.iter().any(|frame| *frame != expected[0]),
        "sine MIDI varies across the routed constant audio contribution"
    );
    let position = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .position
        .load(Acquire);
    let request = if group {
        Request::ReorderTracks {
            project_generation: 0,
            revision: original.state.revision,
            tracks: hex_project::TrackSelectionIds::new(&[midi, audio]).unwrap(),
            before: None,
        }
    } else {
        reorder(&original, midi, Some(audio))
    };
    let moved = run_applied(&mut h, &mut applied, request);
    assert_eq!(
        order(&moved),
        if group {
            vec![empty, audio, midi]
        } else {
            vec![midi, audio, empty]
        }
    );
    assert_eq!(h.audio.as_ref().unwrap().control.output_identity(), output);
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Acquire));
    assert_eq!(status.position.load(Acquire), position + 64);
    // applied() rendered exactly one installation block. Advance the independent
    // original-order graph by that block, then compare the following live block.
    uninterrupted.render(&mut expected).unwrap();
    uninterrupted.render(&mut expected).unwrap();
    assert_eq!(
        h.render(),
        expected,
        "reordering preserves audio plus MIDI phase"
    );
    assert!(pcm.shares_pcm_with(&resident_pcm(&moved)));
    for track in original.state.project.tracks() {
        assert_eq!(moved.state.project.track(track.id()), Some(track));
    }
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    let request = if group {
        Request::ReorderTracks {
            project_generation: 0,
            revision: moved.state.revision,
            tracks: hex_project::TrackSelectionIds::new(&[midi, audio]).unwrap(),
            before: Some(empty),
        }
    } else {
        reorder(&moved, audio, Some(midi))
    };
    h.service.submit(request).unwrap();
    let refused = h.next();
    assert!(
        refused
            .error
            .as_ref()
            .unwrap()
            .contains("fixture final admission refusal")
    );
    assert_eq!(refused.state.project, moved.state.project);
    assert_eq!(refused.state.revision, moved.state.revision);
    assert_eq!(
        (refused.state.can_undo, refused.state.can_redo),
        (moved.state.can_undo, moved.state.can_redo)
    );
    assert!(Arc::ptr_eq(
        refused.state.canonical_audio_source.as_ref().unwrap(),
        moved.state.canonical_audio_source.as_ref().unwrap()
    ));
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        applied
    );
    assert_eq!(h.audio.as_ref().unwrap().control.output_identity(), output);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let undone = run_applied(&mut h, &mut applied, Request::Undo);
    assert_eq!(undone.state.project, original.state.project);
    uninterrupted.render(&mut expected).unwrap();
    let redone = run_applied(&mut h, &mut applied, Request::Redo);
    uninterrupted.render(&mut expected).unwrap();
    assert_eq!(redone.state.project, moved.state.project);
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    assert!(pcm.shares_pcm_with(&resident_pcm(&redone)));
    uninterrupted.render(&mut expected).unwrap();
    assert_eq!(
        h.render(),
        expected,
        "Undo/Redo continues uninterrupted audio plus MIDI phase"
    );
}

/// Controlled callback pumping measures this worker/publication fixture, not physical
/// deadlines, UI latency, or third-party plugin load behavior. Run in release mode.
#[test]
#[ignore = "opt-in optimized 100-track worker/publication latency probe"]
fn reorder_track_latency_100_tracks() {
    let _native = native_fixture();
    let files = Files::new();
    let path = files.wav(48_000, 8192);
    let mut h = ResidentHarness::with_frame_limit(262_144);
    h.next();
    let (mut applied, _) = h.applied(0);
    let mut event = run_applied(&mut h, &mut applied, Request::Import(path.clone()));
    let asset = event.state.project.assets().next().unwrap().id();
    for i in 1..50 {
        event = run_applied(
            &mut h,
            &mut applied,
            Request::Edit(Command::AddTrack {
                name: format!("audio {i}"),
            }),
        );
        let track = event.state.project.tracks().last().unwrap().id();
        event = run_applied(
            &mut h,
            &mut applied,
            Request::Edit(Command::PlaceClip {
                track,
                asset,
                start: 0,
                duration: 8192,
                source_offset: 0,
                source_length: 8192,
                looping: true,
            }),
        );
    }
    for _ in 0..50 {
        event = add_instrument(&mut h, &mut applied, &event);
        let track = event.state.project.tracks().last().unwrap().id();
        event = run_applied(
            &mut h,
            &mut applied,
            Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            }),
        );
        let Some(hex_project::Created::Clip(clip)) = event.created else {
            panic!("clip")
        };
        event = run_applied(
            &mut h,
            &mut applied,
            Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 80,
            }),
        );
    }
    assert_eq!(event.state.project.tracks().len(), 100);
    let ids = order(&event);
    let pcm = resident_pcm(&event);
    fs::remove_file(path).unwrap();
    // Prepare/cache warm-up is excluded. Rotate the first row to the end each time.
    for _ in 0..2 {
        let track = event.state.project.tracks()[0].id();
        let request = reorder(&event, track, None);
        event = run_applied(&mut h, &mut applied, request);
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert!(h.render().iter().flatten().any(|sample| *sample != 0.0));
    let mut canonical_us = Vec::new();
    let mut publication_us = Vec::new();
    for _ in 0..10 {
        let track = event.state.project.tracks()[0].id();
        let request = reorder(&event, track, None);
        let start = Instant::now();
        h.service.submit(request).unwrap();
        event = h.next();
        let terminal = Instant::now();
        assert!(event.error.is_none(), "{:?}", event.error);
        let (revision, metadata) = h.applied(applied);
        applied = revision;
        assert_eq!(metadata.context.editor_revision, event.state.revision);
        canonical_us.push(terminal.duration_since(start).as_micros());
        publication_us.push(terminal.elapsed().as_micros());
    }
    canonical_us.sort_unstable();
    publication_us.sort_unstable();
    let mut final_ids = order(&event);
    final_ids.sort();
    let mut original_ids = ids;
    original_ids.sort();
    assert_eq!(final_ids, original_ids);
    assert!(pcm.shares_pcm_with(&resident_pcm(&event)));
    eprintln!(
        "track reorder probe: profile_debug={} rate=48000 block=64 tracks=100 (50 audio/50 midi,50 sine devices) shared_source_frames=8192 warmup=2 samples=10; submit_to_terminal_us median={} max={}; terminal_to_callback_ack_us median={} max={}; callback manually pumped; no equivalent pre-command baseline",
        cfg!(debug_assertions),
        canonical_us[5],
        canonical_us[9],
        publication_us[5],
        publication_us[9]
    );
}
