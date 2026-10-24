//! Real canonical worker, admission, stopped gate and headless native callback.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::{BeatTime, Command, Created, ElementColor, SectionId, Tempo};

fn section_request(revision: u64, command: Command) -> Request {
    Request::EditArrangementSection {
        project_generation: 0,
        revision,
        command,
    }
}
fn run_callback(h: &mut ResidentHarness, request: Request) -> Event {
    h.service.submit(request).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render();
        if let Ok(event) = h.service.try_event() {
            return event;
        }
        assert!(Instant::now() < until, "section worker did not complete");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn add(
    h: &mut ResidentHarness,
    revision: u64,
    name: &str,
    start: u64,
    end: u64,
) -> (Event, SectionId) {
    let event = h.run(section_request(
        revision,
        Command::AddArrangementSection {
            name: name.into(),
            color: ElementColor::Cyan,
            start: BeatTime::from_quarters(start).unwrap(),
            end: BeatTime::from_quarters(end).unwrap(),
        },
    ));
    let Some(Created::ArrangementSection(id)) = event.created else {
        panic!("section identity");
    };
    (event, id)
}
#[test]
fn resident_sections_reproject_stopped_cursor_and_refuse_stale_or_failed_edits() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let mut h = ResidentHarness::new();
    let initial = h.next();
    assert!(initial.error.is_none());
    let (mut applied, _) = h.applied(0);
    let track = h.run(Request::Edit(Command::AddMidiTrack {
        name: "song".into(),
    }));
    let Some(Created::Track(track_id)) = track.created else {
        panic!("track");
    };
    (applied, _) = h.applied(applied);
    let clip = h.run(Request::Edit(Command::PlaceMidiClip {
        track: track_id,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8).unwrap(),
    }));
    (applied, _) = h.applied(applied);
    let (first, intro) = add(&mut h, clip.state.revision, "intro", 0, 2);
    (applied, _) = h.applied(applied);
    let (second, chorus) = add(&mut h, first.state.revision, "chorus", 4, 7);
    (applied, _) = h.applied(applied);
    assert!(
        second.state.timeline_frames
            >= second
                .state
                .project
                .tempo_map()
                .beat_to_frame(BeatTime::from_quarters(7).unwrap())
                .unwrap()
    );
    let tempo = h.run(Request::Edit(Command::SetTempoPoint {
        beat: BeatTime::from_quarters(4).unwrap(),
        tempo: Tempo::new(90_000_000).unwrap(),
    }));
    (applied, _) = h.applied(applied);
    let before = tempo.state.project.snapshot();
    let before_revision = tempo.state.revision;
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Seek(24_000))
        .unwrap();
    h.render();
    let position = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .position
        .load(Acquire);
    let cursor = tempo
        .state
        .project
        .tempo_map()
        .frame_to_beat(position)
        .unwrap();
    let changed = run_callback(
        &mut h,
        section_request(
            before_revision,
            Command::ReorderSectionAfter {
                section: intro,
                after: chorus,
            },
        ),
    );
    assert!(changed.error.is_none(), "{:?}", changed.error);
    assert_eq!(changed.state.revision, before_revision + 1);
    (applied, _) = h.applied(applied);
    let after_position = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .position
        .load(Acquire);
    assert_eq!(
        after_position,
        changed
            .state
            .project
            .tempo_map()
            .beat_to_frame(cursor)
            .unwrap()
    );
    assert!(
        !h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    let after = changed.state.project.snapshot();
    let stale = run_callback(
        &mut h,
        section_request(before_revision, Command::RemoveArrangementSection(intro)),
    );
    assert!(stale.error.as_ref().is_some_and(|e| e.contains("changed")));
    assert_eq!(stale.state.project.snapshot(), after);
    let malformed = run_callback(
        &mut h,
        section_request(changed.state.revision, Command::RemoveTrack(track_id)),
    );
    assert!(
        malformed
            .error
            .as_ref()
            .is_some_and(|e| e.contains("structural marker"))
    );
    assert_eq!(malformed.state.project.snapshot(), after);
    let undone = run_callback(&mut h, Request::Undo);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    let restored = undone.state.project.snapshot();
    assert!(
        restored.next_id >= before.next_id,
        "history never rewinds identity high-water"
    );
    let mut expected_before = before;
    expected_before.next_id = restored.next_id;
    assert_eq!(restored, expected_before);
    (applied, _) = h.applied(applied);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire),
        position
    );
    let redone = run_callback(&mut h, Request::Redo);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(redone.state.project.snapshot(), after);
    (applied, _) = h.applied(applied);
    let until = Instant::now() + Duration::from_secs(2);
    while h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .stopped_edit_pending()
    {
        h.pump();
        h.render();
        assert!(Instant::now() < until, "installed gate did not retire");
        std::thread::sleep(Duration::from_millis(1));
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.render();
    let tempo_refused = run_callback(
        &mut h,
        section_request(
            redone.state.revision,
            Command::ReorderSectionBefore {
                section: intro,
                before: chorus,
            },
        ),
    );
    assert!(
        tempo_refused
            .error
            .as_ref()
            .is_some_and(|e| e.contains("tempo changes")),
        "{:?}",
        tempo_refused.error
    );
    assert_eq!(tempo_refused.state.project.snapshot(), after);
    assert_eq!(tempo_refused.state.revision, redone.state.revision);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    h.render();
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    let failed = run_callback(
        &mut h,
        section_request(
            redone.state.revision,
            Command::ReorderSectionBefore {
                section: intro,
                before: chorus,
            },
        ),
    );
    assert!(failed.error.is_some());
    assert_eq!(failed.state.project.snapshot(), after);
    assert_eq!(failed.state.revision, redone.state.revision);
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
    // Failed admission must leave the original history edge usable and release
    // the pre-commit gate, rather than merely retain a matching visible snapshot.
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let recovered = run_callback(&mut h, Request::Undo);
    assert!(recovered.error.is_none(), "{:?}", recovered.error);
    let recovered_snapshot = recovered.state.project.snapshot();
    assert!(recovered_snapshot.next_id >= expected_before.next_id);
    expected_before.next_id = recovered_snapshot.next_id;
    assert_eq!(recovered_snapshot, expected_before);
    h.applied(applied);
}

#[test]
fn stateless_section_reorder_retains_late_and_outside_loop_stopped_positions() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    for (position, enabled_loop) in [(9_000_000, false), (96_000, true)] {
        let mut h = ResidentHarness::new();
        let initial = h.next();
        assert!(initial.error.is_none());
        let (mut applied, _) = h.applied(0);
        let (first, intro) = add(&mut h, initial.state.revision, "intro", 0, 2);
        (applied, _) = h.applied(applied);
        let (second, chorus) = add(&mut h, first.state.revision, "chorus", 4, 7);
        (applied, _) = h.applied(applied);
        let state = if enabled_loop {
            let event = h.run(Request::Edit(Command::SetLoopRegion {
                start: 12_000,
                end: 24_000,
                enabled: true,
            }));
            assert!(event.error.is_none());
            (applied, _) = h.applied(applied);
            event.state
        } else {
            second.state
        };
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Seek(position))
            .unwrap();
        h.render();
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .position
                .load(Acquire),
            position
        );
        let changed = run_callback(
            &mut h,
            section_request(
                state.revision,
                Command::ReorderSectionBefore {
                    section: chorus,
                    before: intro,
                },
            ),
        );
        assert!(changed.error.is_none(), "{:?}", changed.error);
        (applied, _) = h.applied(applied);
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .position
                .load(Acquire),
            position
        );
        assert_eq!(
            changed.state.project.loop_region(),
            state.project.loop_region()
        );
        let undone = run_callback(&mut h, Request::Undo);
        assert!(undone.error.is_none(), "{:?}", undone.error);
        h.applied(applied);
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .position
                .load(Acquire),
            position
        );
    }
}

#[test]
fn structural_section_splices_use_stopped_publication_and_preserve_failed_history() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    for kind in [0, 1, 3, 4] {
        let mut h = ResidentHarness::new();
        let initial = h.next();
        assert!(initial.error.is_none());
        let (mut applied, _) = h.applied(0);
        let event = h.run(Request::Edit(Command::AddMidiTrack {
            name: "song".into(),
        }));
        let Some(Created::Track(track)) = event.created else {
            panic!("track")
        };
        (applied, _) = h.applied(applied);
        let event = h.run(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(8).unwrap(),
        }));
        (applied, _) = h.applied(applied);
        let (mut event, section) = add(&mut h, event.state.revision, "intro", 0, 2);
        (applied, _) = h.applied(applied);
        let adjacent = if kind == 3 {
            let (next, id) = add(&mut h, event.state.revision, "next", 2, 4);
            event = next;
            (applied, _) = h.applied(applied);
            Some(id)
        } else {
            None
        };
        let before = event.state.project.snapshot();
        let revision = event.state.revision;
        let command = match kind {
            0 => Command::ResizeArrangementSection {
                section,
                edge: hex_project::TrimEdge::Right,
                length: BeatTime::from_quarters(3).unwrap(),
            },
            1 => Command::DeleteArrangementSection(section),
            3 => Command::ResizeArrangementSectionBoundary {
                left: section,
                right: adjacent.unwrap(),
                boundary: BeatTime::from_quarters(3).unwrap(),
                marker_only: false,
            },
            _ => Command::ResizeArrangementSectionEdge {
                section,
                edge: hex_project::TrimEdge::Right,
                boundary: BeatTime::from_quarters(3).unwrap(),
            },
        };
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        h.render();
        let refused = run_callback(&mut h, section_request(revision, command.clone()));
        assert!(
            refused
                .error
                .as_ref()
                .is_some_and(|e| e.contains("stop playback")),
            "{:?}",
            refused.error
        );
        assert_eq!(refused.state.project.snapshot(), before);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Stop)
            .unwrap();
        h.render();
        h.refuse.store(true, std::sync::atomic::Ordering::Release);
        let refused = run_callback(&mut h, section_request(revision, command.clone()));
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project.snapshot(), before);
        h.refuse.store(false, std::sync::atomic::Ordering::Release);
        let changed = run_callback(&mut h, section_request(revision, command));
        assert_eq!(
            changed.state.project.meter_map(),
            &hex_project::MeterMap::common_time(),
            "stopped section publication must not install synthetic same-meter boundaries"
        );
        assert!(changed.error.is_none(), "{:?}", changed.error);
        (applied, _) = h.applied(applied);
        let after = changed.state.project.snapshot();
        assert_ne!(after, before);
        let undone = run_callback(&mut h, Request::Undo);
        assert!(undone.error.is_none(), "{:?}", undone.error);
        let mut restored = undone.state.project.snapshot();
        restored.next_id = before.next_id;
        assert_eq!(restored, before);
        (applied, _) = h.applied(applied);
        let redone = run_callback(&mut h, Request::Redo);
        assert!(redone.error.is_none(), "{:?}", redone.error);
        assert_eq!(redone.state.project.snapshot(), after);
        h.applied(applied);
    }
}

/// Native worker, prospective admission, publication and callback stay real.
/// Every render also probes zero total and signed-net callback allocations.
#[test]
fn live_section_moves_duplicates_and_history_keep_advancing_audio_and_atomic_refusal() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(8_000_000);
    let initial = h.next();
    assert!(initial.error.is_none());
    let (mut applied, _) = h.applied(0);
    let path = files.wav(48_000, 960_000);
    let mut bytes = fs::read(&path).unwrap();
    let mut frames = Vec::with_capacity(960_000);
    for (i, sample) in bytes[44..].chunks_exact_mut(2).enumerate() {
        let value: i16 = match i {
            0..48_000 => 8192,
            48_000..96_000 => 16384,
            _ => -8192,
        };
        sample.copy_from_slice(&value.to_le_bytes());
        frames.push([f32::from(value) / 32768.0; 2]);
    }
    fs::write(&path, bytes).unwrap();
    let imported = h.run(Request::Import(path));
    assert!(imported.error.is_none(), "{:?}", imported.error);
    (applied, _) = h.applied(applied);
    let audio = hex_engine::Audio::new(48_000, frames).unwrap();
    let asset = imported.state.project.tracks()[0].clips()[0].asset();
    let media = std::collections::BTreeMap::from([(asset, audio)]);
    let (first, intro) = add(&mut h, imported.state.revision, "intro", 0, 2);
    (applied, _) = h.applied(applied);
    let (mut event, chorus) = add(&mut h, first.state.revision, "chorus", 4, 7);
    (applied, _) = h.applied(applied);
    // Reproduce the user's "section launch applied" predecessor, whose loop
    // exists only in the actual runtime, never in the saved project.
    let loop_region = hex_project::LoopRegion::new(0, 48_000, true).unwrap();
    let mut intent = h.seek_intent(1, event.state.revision, None, None);
    intent.destination_frame = 0;
    intent.section = Some(hex_session::arrangement_seek::SectionLaunch {
        region: loop_region,
        quantization: hex_engine::LaunchQuantization::Immediate,
    });
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    assert!(h.seek_result().result.is_ok());
    (applied, _) = h.applied(applied);
    assert!(event.state.project.loop_region().is_none());
    assert!(h.render().iter().all(|f| f[0] != 0.0));
    for command in [
        Command::ReorderSectionAfter {
            section: intro,
            after: chorus,
        },
        Command::ReorderSectionBefore {
            section: intro,
            before: chorus,
        },
        Command::DuplicateArrangementSection(intro),
    ] {
        let before = event.state.project.snapshot();
        let revision = event.state.revision;
        let position = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire);
        event = run_callback(&mut h, section_request(revision, command.clone()));
        assert!(event.error.is_none(), "{:?}", event.error);
        (applied, _) = h.applied(applied);
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(status.playing.load(Acquire));
        assert!(
            status.position.load(Acquire) > position,
            "must inherit the advancing installation cursor"
        );
        let cursor = status.position.load(Acquire);
        let prepared = Arc::new(
            hex_engine::PreparedRevision::prepare(
                &event.state.project,
                &media,
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut oracle =
            hex_engine::playback::Playback::try_from_prepared_with_session(prepared).unwrap();
        oracle.apply(PlaybackCommand::Seek(cursor));
        oracle.apply(PlaybackCommand::Play);
        let mut expected = [[0.0; 2]; 64];
        oracle.render(&mut expected).unwrap();
        assert_eq!(
            h.render(),
            expected,
            "installed arrangement must sound at the actual cursor"
        );
        let after = event.state.project.snapshot();
        assert_ne!(after, before);
        event = run_callback(&mut h, Request::Undo);
        assert!(event.error.is_none(), "{:?}", event.error);
        (applied, _) = h.applied(applied);
        let mut restored = event.state.project.snapshot();
        restored.next_id = before.next_id;
        assert_eq!(restored, before);
        assert!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire)
        );
        event = run_callback(&mut h, Request::Redo);
        assert!(event.error.is_none(), "{:?}", event.error);
        assert_eq!(event.state.project.snapshot(), after);
        (applied, _) = h.applied(applied);
        assert!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire)
        );
        assert!(h.render().iter().all(|f| f[0] != 0.0));
        h.refuse.store(true, std::sync::atomic::Ordering::Release);
        let refused = run_callback(
            &mut h,
            section_request(
                event.state.revision,
                Command::DuplicateArrangementSection(intro),
            ),
        );
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project.snapshot(), after);
        assert_eq!(refused.state.revision, event.state.revision);
        h.refuse.store(false, std::sync::atomic::Ordering::Release);
        assert!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire)
        );
    }
    for _ in 0..800 {
        h.render();
    }
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Acquire));
    assert!(
        status.position.load(Acquire) < loop_region.end(),
        "live section edits must keep the launched loop"
    );
    assert!(
        event.state.project.loop_region().is_none(),
        "the launched loop stays transient"
    );
}

#[test]
fn live_section_edits_retain_a_sounding_sampler_and_its_launched_loop() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(8_000_000);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        empty_sampler: None,
        path: files.wav(48_000, 192_000),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    assert!(loaded.error.is_none());
    (applied, _) = h.applied(applied);
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8).unwrap(),
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    (applied, _) = h.applied(applied);
    let notes = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(6).unwrap(),
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    let (first, intro) = add(&mut h, notes.state.revision, "intro", 0, 2);
    (applied, _) = h.applied(applied);
    let (mut event, chorus) = add(&mut h, first.state.revision, "chorus", 4, 7);
    (applied, _) = h.applied(applied);
    let mut intent = h.seek_intent(2, event.state.revision, None, None);
    intent.destination_frame = 0;
    intent.section = Some(hex_session::arrangement_seek::SectionLaunch {
        region: hex_project::LoopRegion::new(0, 48_000, true).unwrap(),
        quantization: hex_engine::LaunchQuantization::Immediate,
    });
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    assert!(h.seek_result().result.is_ok());
    (applied, _) = h.applied(applied);
    assert!(h.render().iter().flatten().any(|s| *s != 0.0));
    for command in [
        Command::DuplicateArrangementSection(intro),
        Command::ReorderSectionAfter {
            section: intro,
            after: chorus,
        },
    ] {
        let before = event.state.project.snapshot();
        let unchanged_prefix = matches!(&command, Command::DuplicateArrangementSection(_));
        let before_pcm = h.render();
        event = run_callback(&mut h, section_request(event.state.revision, command));
        assert!(event.error.is_none(), "{:?}", event.error);
        (applied, _) = h.applied(applied);
        assert!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire)
        );
        let heard = h.render();
        assert!(heard.iter().flatten().any(|s| *s != 0.0));
        if unchanged_prefix {
            assert_eq!(
                heard, before_pcm,
                "duplicating later music must not double the held prefix note"
            );
        }
        event = run_callback(&mut h, Request::Undo);
        assert!(event.error.is_none(), "{:?}", event.error);
        (applied, _) = h.applied(applied);
        let mut restored = event.state.project.snapshot();
        restored.next_id = before.next_id;
        assert_eq!(restored, before);
    }
    for _ in 0..800 {
        h.render();
    }
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire)
            < 48_000
    );
    assert!(h.render().iter().flatten().any(|s| *s != 0.0));
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    assert!(h.render().iter().flatten().all(|s| *s == 0.0));
}
