//! Real canonical worker, admission, stopped gate and headless native callback.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::{BeatTime, Command, Created, ElementColor, SectionId, Tempo, TrackId};

fn section_request(revision: u64, command: Command) -> Request {
    Request::EditArrangementSection {
        project_generation: 0,
        revision,
        command,
    }
}
pub(super) fn run_callback(h: &mut ResidentHarness, request: Request) -> Event {
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
    let _native = native_fixture();
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
    // A section reorder that changes the tempo map while playing used to
    // refuse ("tempo map"). It now applies live, relocating audio to the same
    // beat under the new map (Pulse #3774); Undo applies live as well.
    let tempo_moved = run_callback(
        &mut h,
        section_request(
            redone.state.revision,
            Command::ReorderSectionBefore {
                section: intro,
                before: chorus,
            },
        ),
    );
    assert!(tempo_moved.error.is_none(), "{:?}", tempo_moved.error);
    assert_ne!(tempo_moved.state.project.snapshot(), after);
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
    let tempo_undone = run_callback(&mut h, Request::Undo);
    assert!(tempo_undone.error.is_none(), "{:?}", tempo_undone.error);
    (applied, _) = h.applied(applied);
    let mut tempo_restored = tempo_undone.state.project.snapshot();
    tempo_restored.next_id = after.next_id;
    assert_eq!(tempo_restored, after);
    let after = tempo_undone.state.project.snapshot();
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
            tempo_undone.state.revision,
            Command::ReorderSectionBefore {
                section: intro,
                before: chorus,
            },
        ),
    );
    assert!(failed.error.is_some());
    assert_eq!(failed.state.project.snapshot(), after);
    assert_eq!(failed.state.revision, tempo_undone.state.revision);
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
    let _native = native_fixture();
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
fn structural_section_splices_apply_live_or_stopped_and_preserve_failed_history() {
    let _native = native_fixture();
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
        // Structural section edits apply while playing (Pulse #3784); the
        // transport keeps running and Undo applies live too.
        let live = run_callback(&mut h, section_request(revision, command.clone()));
        assert!(live.error.is_none(), "kind={kind}: {:?}", live.error);
        assert_ne!(live.state.project.snapshot(), before);
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
        let live_undo = run_callback(&mut h, Request::Undo);
        assert!(live_undo.error.is_none(), "{:?}", live_undo.error);
        (applied, _) = h.applied(applied);
        let mut live_restored = live_undo.state.project.snapshot();
        live_restored.next_id = before.next_id;
        assert_eq!(live_restored, before);
        let before = live_undo.state.project.snapshot();
        let revision = live_undo.state.revision;
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
    let _native = native_fixture();
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
    let (_, chorus) = add(&mut h, first.state.revision, "chorus", 4, 7);
    (applied, _) = h.applied(applied);
    // An enabled section launch must match the canonical project loop.
    let loop_region = hex_project::LoopRegion::new(0, 48_000, true).unwrap();
    let mut event = h.run(Request::Edit(Command::SetLoopRegion {
        start: loop_region.start(),
        end: loop_region.end(),
        enabled: true,
    }));
    assert!(event.error.is_none(), "{:?}", event.error);
    (applied, _) = h.applied(applied);
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
    assert_eq!(event.state.project.loop_region(), Some(loop_region));
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
        Command::DuplicateArrangementSectionAt {
            section: intro,
            target: chorus,
            after: true,
        },
        Command::DuplicateArrangementSectionAt {
            section: intro,
            target: chorus,
            after: false,
        },
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
        if matches!(command, Command::DuplicateArrangementSectionAt { .. }) {
            let Some(Created::ArrangementSection(copy)) = event.created else {
                panic!("destination copy identity");
            };
            let marker = event.state.project.arrangement_section(copy).unwrap();
            let start = event
                .state
                .project
                .tempo_map()
                .beat_to_frame(marker.start())
                .unwrap();
            let mut copied = hex_engine::playback::Playback::try_from_prepared_with_session(
                Arc::clone(&prepared),
            )
            .unwrap();
            copied.apply(PlaybackCommand::Seek(start));
            copied.apply(PlaybackCommand::Play);
            let mut literal = [[0.0; 2]; 64];
            copied.render(&mut literal).unwrap();
            assert_eq!(
                literal, [[0.25; 2]; 64],
                "copy must read the original intro's PCM, not destination music"
            );
        }
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
                if matches!(command, Command::DuplicateArrangementSectionAt { .. }) {
                    command.clone()
                } else {
                    Command::DuplicateArrangementSection(intro)
                },
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
    assert_eq!(
        event.state.project.loop_region(),
        Some(loop_region),
        "the canonical launched loop stays intact"
    );
}

fn live_section_edits_retain_sampler(micro_bpm: u32, copy_before_self: bool) {
    let _native = native_fixture();
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
    // Original periodic PCM supplies an independent phase/amplitude oracle
    // at the actual content position, including non-quantum-aligned wraps.
    let source_frames = 64_000;
    let sampler_path = files.wav(48_000, source_frames);
    let mut bytes = fs::read(&sampler_path).unwrap();
    for (i, sample) in bytes[44..].chunks_exact_mut(2).enumerate() {
        let value =
            (8192.0 * (std::f64::consts::TAU * (i % 64) as f64 / 64.0).sin()).round() as i16;
        sample.copy_from_slice(&value.to_le_bytes());
    }
    let source_period: [hex_engine::Frame; 64] = std::array::from_fn(|i| {
        let offset = 44 + i * 2;
        let sample = f32::from(i16::from_le_bytes([bytes[offset], bytes[offset + 1]])) / 32768.0;
        [sample; 2]
    });
    fs::write(&sampler_path, bytes).unwrap();
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        slot: hex_session::service::SamplerLoadSlot::New(
            hex_project::InstrumentLoadPlacement::First,
        ),
        path: sampler_path,
        cancel: Arc::new(AtomicBool::new(false)),
    });
    assert!(loaded.error.is_none());
    (applied, _) = h.applied(applied);
    h.run(Request::Edit(Command::SetProjectTempo {
        tempo: Tempo::new(micro_bpm).unwrap(),
    }));
    (applied, _) = h.applied(applied);
    // Authored same-BPM points still move with sections. At180BPM these
    // whole-beat seams have an exactly equal forward/inverse sample clock.
    if micro_bpm == 180_000_000 {
        h.run(Request::Edit(Command::SetTempoPoint {
            beat: BeatTime::from_quarters(3).unwrap(),
            tempo: Tempo::new(micro_bpm).unwrap(),
        }));
        (applied, _) = h.applied(applied);
    }
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
    let loop_frames = event
        .state
        .project
        .tempo_map()
        .beat_to_frame(BeatTime::from_quarters(2).unwrap())
        .unwrap();
    // Every pass after the first also carries the previous pass's voice,
    // gated at the seam, through the prepared ~2 ms declick floor (96
    // frames at 48 kHz). The loop is a whole number of source periods,
    // so that tail keeps the same phase. These checks all run after the
    // first seam whenever the position is inside that tail.
    let release_floor = 96;
    assert!(u64::from(source_frames) > loop_frames + release_floor);
    let expected_from_start = |start: u64| -> [hex_engine::Frame; 64] {
        std::array::from_fn(|i| {
            let position = (start + i as u64) % loop_frames;
            let frame = source_period[(position % 64) as usize];
            let tail = 1.0 - position.min(release_floor) as f32 / release_floor as f32;
            [frame[0] * (1.0 + tail), frame[1] * (1.0 + tail)]
        })
    };
    // Two summed voices round differently from the single-voice product.
    let assert_near = |actual: [hex_engine::Frame; 64], start: u64, context: &str| {
        let expected = expected_from_start(start);
        for (index, (a, e)) in actual.iter().zip(&expected).enumerate() {
            assert!(
                (a[0] - e[0]).abs() < 1e-6 && (a[1] - e[1]).abs() < 1e-6,
                "{context} frame {index} from {start}: {a:?} != {e:?}"
            );
        }
    };
    let expected_at = |end: u64| {
        let start = (end + loop_frames - 64) % loop_frames;
        std::array::from_fn::<_, 64, _>(|index| {
            source_period[((start + index as u64) % loop_frames % 64) as usize]
        })
    };
    // A looped section launch now follows the same canonical loop edit as
    // the L-held UI gesture; wait for its publication before launching.
    event = h.run(Request::Edit(Command::SetLoopRegion {
        start: 0,
        end: loop_frames,
        enabled: true,
    }));
    (applied, _) = h.applied(applied);
    let mut intent = h.seek_intent(2, event.state.revision, None, None);
    intent.destination_frame = 0;
    intent.section = Some(hex_session::arrangement_seek::SectionLaunch {
        region: hex_project::LoopRegion::new(0, loop_frames, true).unwrap(),
        quantization: hex_engine::LaunchQuantization::Immediate,
    });
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    assert!(h.seek_result().result.is_ok());
    (applied, _) = h.applied(applied);
    assert!(h.render().iter().flatten().any(|s| *s != 0.0));
    // Identical A/B sounds advance canonical history without a new audio
    // binding. The next ordinary edit may therefore skip source revisions.
    let instrument = event
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap();
    let before_comparison = h.render();
    let status = h.audio.as_ref().unwrap().status().unwrap();
    let comparison_clock = (
        status.position.load(Acquire),
        status.performance_frame.load(Acquire),
    );
    assert_eq!(before_comparison, expected_at(comparison_clock.0));
    event = h.run(Request::CompareDevice {
        owner: hex_session::service::preset::Owner {
            project_generation: 0,
            revision: event.state.revision,
            device: instrument.id(),
            class: hex_engine::NativeDeviceClass::Instrument,
            type_id: instrument.state().type_id(),
        },
        expected: hex_project::DeviceVariant::A,
        action: hex_project::DeviceComparisonAction::Switch,
    });
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
    let compared = h.render();
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert_eq!(
        status.performance_frame.load(Acquire),
        comparison_clock.1 + 64
    );
    assert_eq!(
        status.position.load(Acquire),
        (comparison_clock.0 + 64) % loop_frames
    );
    assert_eq!(compared, expected_at(status.position.load(Acquire)));
    let before_noop = event.state.project.snapshot();
    let noop = run_callback(
        &mut h,
        section_request(
            event.state.revision,
            Command::ReorderSectionBefore {
                section: intro,
                before: intro,
            },
        ),
    );
    assert!(noop.error.is_none(), "{:?}", noop.error);
    assert_eq!(noop.state.revision, event.state.revision);
    assert_eq!(noop.state.project.snapshot(), before_noop);
    for command in [
        Command::RenameArrangementSection {
            section: intro,
            name: "renamed".into(),
        },
        Command::SetArrangementSectionColor {
            section: intro,
            color: ElementColor::Rose,
        },
        Command::MoveArrangementSectionMarker {
            section: intro,
            start: BeatTime::from_quarters(5).unwrap(),
        },
        Command::ResizeArrangementSectionMarker {
            section: intro,
            edge: hex_project::TrimEdge::Right,
            boundary: BeatTime::from_quarters(5).unwrap(),
        },
        Command::RenameTrack {
            track,
            name: "metadata".into(),
        },
        Command::DuplicateArrangementSection(intro),
    ] {
        let before = event.state.project.snapshot();
        let unchanged_prefix = matches!(&command, Command::DuplicateArrangementSection(_))
            || !command.requires_section_publication();
        if matches!(&command, Command::RenameArrangementSection { .. }) {
            // Deterministically cross a real non-quantum-aligned seam during
            // publication, rather than depending on worker/compiler speed.
            for _ in 0..loop_frames.div_ceil(64) {
                if h.audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .position
                    .load(Acquire)
                    + 128
                    >= loop_frames
                {
                    break;
                }
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
                    + 128
                    >= loop_frames,
                "callback must reach the seam within one admitted loop"
            );
        }
        let before_frame = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire);
        let before_pcm = h.render();
        let before_status = h.audio.as_ref().unwrap().status().unwrap();
        let before_clock = (
            before_status.position.load(Acquire),
            before_status.performance_frame.load(Acquire),
        );
        if unchanged_prefix {
            assert_near(before_pcm, before_frame, "before");
            assert_eq!(before_pcm, expected_at(before_clock.0));
        }
        let request = if matches!(&command, Command::RenameTrack { .. }) {
            Request::Edit(command)
        } else {
            section_request(event.state.revision, command)
        };
        event = run_callback(&mut h, request);
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
        let heard_frame = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire);
        let heard = h.render();
        assert!(heard.iter().flatten().any(|s| *s != 0.0));
        if unchanged_prefix {
            assert_near(heard, heard_frame, "heard");
            let status = h.audio.as_ref().unwrap().status().unwrap();
            let after_clock = (
                status.position.load(Acquire),
                status.performance_frame.load(Acquire),
            );
            assert!(after_clock.1 >= before_clock.1 + 64);
            assert_eq!(
                after_clock.0,
                (before_clock.0 + after_clock.1 - before_clock.1) % loop_frames
            );
            assert_near(
                heard,
                (after_clock.0 + loop_frames - 64) % loop_frames,
                &format!(
                    "metadata or duplicating later music must retain exact source phase and amplitude at the continuous loop clock: before={before_clock:?}, after={after_clock:?}, loop={loop_frames}"
                ),
            );
        }
        // A new loop pass dispatches the shortened prefix signature. Undo
        // must work after this boundary, independently of worker timing.
        if unchanged_prefix && event.state.project.arrangement_sections().len() > 2 {
            for _ in 0..loop_frames.div_ceil(64) + 1 {
                h.render();
            }
        }
        event = run_callback(&mut h, Request::Undo);
        assert!(event.error.is_none(), "{:?}", event.error);
        (applied, _) = h.applied(applied);
        let mut restored = event.state.project.snapshot();
        restored.next_id = before.next_id;
        assert_eq!(restored, before);
        if unchanged_prefix {
            let start = h
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .position
                .load(Acquire);
            assert_near(
                h.render(),
                start,
                "Undo must preserve the original sampler phase without a second NoteOn",
            );
        }
    }
    let original = event.state.project.snapshot();

    let relocation = if copy_before_self {
        Command::DuplicateArrangementSectionAt {
            section: intro,
            target: intro,
            after: false,
        }
    } else {
        Command::ReorderSectionAfter {
            section: intro,
            after: chorus,
        }
    };
    // Relocating sounding sampler MIDI while playing used to refuse with
    // "stop playback". It now applies live (Pulse #3774): audio restarts at
    // the cursor, the launched loop keeps running and the sampler sounds.
    let moved = run_callback(&mut h, section_request(event.state.revision, relocation));
    assert!(moved.error.is_none(), "{:?}", moved.error);
    assert_ne!(moved.state.project.snapshot(), original);
    (applied, _) = h.applied(applied);
    let audible_loop = |h: &mut ResidentHarness| {
        let mut heard = false;
        for _ in 0..loop_frames.div_ceil(64) + 1 {
            heard |= h.render().iter().flatten().any(|s| *s != 0.0);
        }
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(status.playing.load(Acquire), "transport keeps running");
        assert!(
            status.position.load(Acquire) < loop_frames,
            "launched loop kept"
        );
        assert!(status.rejected_revision().is_none());
        heard
    };
    assert!(audible_loop(&mut h));
    let undone = run_callback(&mut h, Request::Undo);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    (applied, _) = h.applied(applied);
    let mut restored = undone.state.project.snapshot();
    restored.next_id = original.next_id;
    assert_eq!(restored, original);
    assert!(audible_loop(&mut h));
    let redone = run_callback(&mut h, Request::Redo);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    h.applied(applied);
    assert_eq!(
        redone.state.project.snapshot(),
        moved.state.project.snapshot()
    );
    assert!(audible_loop(&mut h));
}

#[test]
fn live_section_edits_retain_sampler_at_143_bpm() {
    live_section_edits_retain_sampler(143_000_000, false);
}

#[test]
fn live_section_edits_retain_sampler_at_180_bpm() {
    live_section_edits_retain_sampler(180_000_000, false);
}

#[test]
fn live_section_copy_before_self_retains_sampler() {
    live_section_edits_retain_sampler(143_000_000, true);
}

#[test]
fn empty_section_structural_history_retains_the_launched_loop_while_playing() {
    let _native = native_fixture();
    let mut h = ResidentHarness::new();
    let initial = h.next();
    let (mut applied, _) = h.applied(0);
    let (first, intro) = add(&mut h, initial.state.revision, "intro", 0, 2);
    (applied, _) = h.applied(applied);
    let (_, chorus) = add(&mut h, first.state.revision, "chorus", 4, 7);
    (applied, _) = h.applied(applied);
    let mut event = h.run(Request::Edit(Command::SetLoopRegion {
        start: 0,
        end: 48_000,
        enabled: true,
    }));
    assert!(event.error.is_none(), "{:?}", event.error);
    (applied, _) = h.applied(applied);
    let mut intent = h.seek_intent(1, event.state.revision, None, None);
    intent.destination_frame = 0;
    intent.section = Some(hex_session::arrangement_seek::SectionLaunch {
        region: hex_project::LoopRegion::new(0, 48_000, true).unwrap(),
        quantization: hex_engine::LaunchQuantization::Immediate,
    });
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    assert!(h.seek_result().result.is_ok());
    (applied, _) = h.applied(applied);
    for command in [
        Command::ReorderSectionAfter {
            section: intro,
            after: chorus,
        },
        Command::DuplicateArrangementSection(intro),
    ] {
        for request in [
            section_request(event.state.revision, command),
            Request::Undo,
            Request::Redo,
        ] {
            event = run_callback(&mut h, request);
            assert!(event.error.is_none(), "{:?}", event.error);
            (applied, _) = h.applied(applied);
            for _ in 0..800 {
                h.render();
            }
            let status = h.audio.as_ref().unwrap().status().unwrap();
            assert!(status.playing.load(Acquire));
            assert!(
                status.position.load(Acquire) < 48_000,
                "even an empty structural history edge must keep the launched loop"
            );
        }
    }
}
struct MetadataLoop {
    h: ResidentHarness,
    event: Event,
    intro: SectionId,
    track: TrackId,
    applied: u64,
    region: hex_project::LoopRegion,
}
fn metadata_loop_fixture() -> MetadataLoop {
    let mut h = ResidentHarness::with_frame_limit(8_000_000);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let track = h.run(Request::Edit(Command::AddTrack {
        name: "metadata".into(),
    }));
    let Some(Created::Track(track_id)) = track.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    let (first, intro) = add(&mut h, track.state.revision, "intro", 0, 2);
    (applied, _) = h.applied(applied);
    add(&mut h, first.state.revision, "chorus", 4, 7);
    (applied, _) = h.applied(applied);
    let region = hex_project::LoopRegion::new(0, 48_000, true).unwrap();
    let event = h.run(Request::Edit(Command::SetLoopRegion {
        start: region.start(),
        end: region.end(),
        enabled: true,
    }));
    assert!(event.error.is_none(), "{:?}", event.error);
    (applied, _) = h.applied(applied);
    let mut intent = h.seek_intent(1, event.state.revision, None, None);
    intent.destination_frame = 0;
    intent.section = Some(hex_session::arrangement_seek::SectionLaunch {
        region,
        quantization: hex_engine::LaunchQuantization::Immediate,
    });
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    assert!(h.seek_result().result.is_ok());
    (applied, _) = h.applied(applied);
    MetadataLoop {
        h,
        event,
        intro,
        track: track_id,
        applied,
        region,
    }
}
fn assert_loop_wraps(h: &ResidentHarness, region: hex_project::LoopRegion) {
    for _ in 0..800 {
        h.render();
    }
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Acquire));
    assert!(
        status.position.load(Acquire) < region.end(),
        "ordinary metadata publication discarded the applied section loop"
    );
}
#[test]
fn ordinary_metadata_and_history_preserve_the_applied_section_loop() {
    let _native = native_fixture();
    let MetadataLoop {
        mut h,
        mut event,
        intro,
        track,
        mut applied,
        region,
    } = metadata_loop_fixture();
    for command in [
        Command::RenameArrangementSection {
            section: intro,
            name: "renamed".into(),
        },
        Command::SetArrangementSectionColor {
            section: intro,
            color: ElementColor::Rose,
        },
        Command::MoveArrangementSectionMarker {
            section: intro,
            start: BeatTime::from_quarters(3).unwrap(),
        },
        Command::ResizeArrangementSectionMarker {
            section: intro,
            edge: hex_project::TrimEdge::Right,
            boundary: BeatTime::from_quarters(6).unwrap(),
        },
        Command::RenameTrack {
            track,
            name: "ordinary metadata".into(),
        },
    ] {
        let before = event.state.project.snapshot();
        let request = if matches!(&command, Command::RenameTrack { .. }) {
            Request::Edit(command)
        } else {
            section_request(event.state.revision, command)
        };
        event = h.run(request);
        assert!(event.error.is_none(), "{:?}", event.error);
        let after = event.state.project.snapshot();
        for (request, expected) in [
            (None, &after),
            (Some(Request::Undo), &before),
            (Some(Request::Redo), &after),
        ] {
            if let Some(request) = request {
                event = h.run(request);
                assert!(event.error.is_none(), "{:?}", event.error);
            }
            (applied, _) = h.applied(applied);
            assert_eq!(&event.state.project.snapshot(), expected);
            assert_loop_wraps(&h, region);
            assert_eq!(event.state.project.loop_region(), Some(region));
        }
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    assert!(h.render().iter().flatten().all(|s| *s == 0.0));
}

#[test]
fn stale_loop_witnesses_and_admission_refusals_preserve_project_redo_and_playback() {
    let _native = native_fixture();
    let MetadataLoop {
        mut h,
        event,
        intro,
        mut applied,
        region,
        ..
    } = metadata_loop_fixture();
    let renamed = h.run(section_request(
        event.state.revision,
        Command::RenameArrangementSection {
            section: intro,
            name: "redo name".into(),
        },
    ));
    (applied, _) = h.applied(applied);
    let undone = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert!(undone.state.can_redo);
    let before = undone.state.project.snapshot();
    // Output, audio revision, source generation/revision and both loop forms.
    for fault in 1..=8 {
        h.loop_witness_fault.store(
            if fault <= 6 { fault } else { 0 },
            std::sync::atomic::Ordering::Release,
        );
        h.refuse
            .store(fault == 7, std::sync::atomic::Ordering::Release);
        h.budget_refuse
            .store(fault == 8, std::sync::atomic::Ordering::Release);
        h.service
            .submit(section_request(
                undone.state.revision,
                Command::RenameArrangementSection {
                    section: intro,
                    name: "must refuse".into(),
                },
            ))
            .unwrap();
        let refused = h.next();
        assert!(refused.error.is_some(), "fault {fault}");
        assert_eq!(refused.state.revision, undone.state.revision);
        assert_eq!(refused.state.project.snapshot(), before);
        assert!(refused.state.can_redo);
        assert_loop_wraps(&h, region);
    }
    h.loop_witness_fault
        .store(0, std::sync::atomic::Ordering::Release);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    h.budget_refuse
        .store(false, std::sync::atomic::Ordering::Release);
    let restored = h.run(Request::Redo);
    h.applied(applied);
    assert_eq!(
        restored.state.project.snapshot(),
        renamed.state.project.snapshot()
    );
    assert_loop_wraps(&h, region);
}

#[test]
fn offline_metadata_edits_and_history_do_not_require_an_applied_loop_witness() {
    let _native = native_fixture();
    let mut h = ResidentHarness::with_open_gate(8192, Arc::new(AtomicBool::new(false)));
    let initial = h.next();
    let (added, intro) = add(&mut h, initial.state.revision, "offline", 0, 2);
    let renamed = h.run(section_request(
        added.state.revision,
        Command::RenameArrangementSection {
            section: intro,
            name: "still editable".into(),
        },
    ));
    let undone = h.run(Request::Undo);
    assert_eq!(
        undone.state.project.snapshot(),
        added.state.project.snapshot()
    );
    let redone = h.run(Request::Redo);
    assert_eq!(
        redone.state.project.snapshot(),
        renamed.state.project.snapshot()
    );
    assert!(h.audio.is_none());
    assert!(h.submitted.is_none());
}

/// Regression for #3917: with no output there is no callback to fence, so a
/// structural edit and its history complete from control state instead of
/// refusing, and the retained result installs when an output opens.
#[test]
fn structural_section_edits_complete_without_an_output_then_install_on_restart() {
    let _native = native_fixture();
    let gate = Arc::new(AtomicBool::new(false));
    let mut h = ResidentHarness::with_open_gate(8192, Arc::clone(&gate));
    let initial = h.next();
    let (added, intro) = add(&mut h, initial.state.revision, "offline", 0, 2);
    let started = Instant::now();
    let duplicated = h.run(section_request(
        added.state.revision,
        Command::DuplicateArrangementSection(intro),
    ));
    // No fence poll: well under the session's 100 ms acknowledgement deadline.
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(duplicated.state.project.arrangement_sections().len(), 2);
    let undone = h.run(Request::Undo);
    assert_eq!(undone.state.project.arrangement_sections().len(), 1);
    let redone = h.run(Request::Redo);
    assert_eq!(
        redone.state.project.snapshot(),
        duplicated.state.project.snapshot()
    );
    assert!(h.audio.is_none() && h.submitted.is_none());
    assert!(h.pending_present.load(Acquire));
    gate.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(Request::RestartAudio {
            project_generation: 0,
            request_id: 1,
        })
        .unwrap();
    let restarted = h.next();
    assert!(restarted.error.is_none(), "{:?}", restarted.error);
    h.wait_fresh_output(0);
    let (_, metadata) = h.applied(0);
    assert_eq!(metadata.context.editor_revision, redone.state.revision);
}

/// Regression for #3917: a latched output fault refuses a stopped edit at
/// once with a recoverable error instead of waiting on a fence.
#[test]
fn latched_output_faults_refuse_section_edits_promptly_and_keep_the_project() {
    let _native = native_fixture();
    let mut h = ResidentHarness::new();
    let initial = h.next();
    let (applied, _) = h.applied(0);
    let (added, intro) = add(&mut h, initial.state.revision, "intro", 0, 2);
    h.applied(applied);
    for fault in [true, false] {
        let status = h.audio.as_ref().unwrap().status().unwrap();
        status
            .device_fault
            .store(fault, std::sync::atomic::Ordering::Release);
        status
            .render_fault
            .store(!fault, std::sync::atomic::Ordering::Release);
        let started = Instant::now();
        h.service
            .submit(section_request(
                added.state.revision,
                Command::DuplicateArrangementSection(intro),
            ))
            .unwrap();
        let refused = h.next();
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "{:?}",
            started.elapsed()
        );
        let error = refused.error.as_deref().expect("fault refusal");
        assert!(error.contains("restart audio"), "{error}");
        assert_eq!(error, error.to_lowercase());
        assert_eq!(
            refused.state.project.snapshot(),
            added.state.project.snapshot()
        );
    }
}

/// Submit an immediate section launch and leave it unapplied by the callback.
fn submit_pending_launch(h: &mut ResidentHarness, request_id: u64, revision: u64) {
    let mut intent = h.seek_intent(request_id, revision, None, None);
    intent.destination_frame = 0;
    intent.section = Some(hex_session::arrangement_seek::SectionLaunch {
        region: hex_project::LoopRegion::new(0, 48_000, true).unwrap(),
        quantization: hex_engine::LaunchQuantization::Immediate,
    });
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    assert!(h.seek_result().result.is_ok()); // Submitted, deliberately not applied.
}

#[test]
fn pending_launch_refuses_undo_and_redo_without_losing_either_history_stack() {
    let _native = native_fixture();
    let MetadataLoop {
        mut h,
        event,
        intro,
        mut applied,
        ..
    } = metadata_loop_fixture();
    let original = event.state.project.snapshot();
    let renamed = h.run(section_request(
        event.state.revision,
        Command::RenameArrangementSection {
            section: intro,
            name: "history target".into(),
        },
    ));
    (applied, _) = h.applied(applied);
    let after = renamed.state.project.snapshot();

    submit_pending_launch(&mut h, 2, renamed.state.revision);
    h.service.submit(Request::Undo).unwrap();
    let refused = h.next();
    assert!(
        refused
            .error
            .as_ref()
            .is_some_and(|e| e.contains("publication already pending")),
        "{:?}",
        refused.error
    );
    assert_eq!(refused.state.revision, renamed.state.revision);
    assert_eq!(refused.state.project.snapshot(), after);
    assert!(refused.state.can_undo);
    (applied, _) = h.applied(applied);
    let undone = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert_eq!(undone.state.project.snapshot(), original);
    assert!(undone.state.can_redo);

    submit_pending_launch(&mut h, 3, undone.state.revision);
    h.service.submit(Request::Redo).unwrap();
    let refused = h.next();
    assert!(
        refused
            .error
            .as_ref()
            .is_some_and(|e| e.contains("publication already pending")),
        "{:?}",
        refused.error
    );
    assert_eq!(refused.state.revision, undone.state.revision);
    assert_eq!(refused.state.project.snapshot(), original);
    assert!(refused.state.can_redo);
    (applied, _) = h.applied(applied);
    let redone = h.run(Request::Redo);
    h.applied(applied);
    assert_eq!(redone.state.project.snapshot(), after);
}

#[test]
fn pending_launch_refuses_metadata_and_stop_cancellation_keeps_the_actual_loop() {
    let _native = native_fixture();
    let MetadataLoop {
        mut h,
        event,
        intro,
        mut applied,
        region,
        ..
    } = metadata_loop_fixture();
    let before = event.state.project.snapshot();
    let mut intent = h.seek_intent(2, event.state.revision, None, None);
    intent.destination_frame = 0;
    intent.section = Some(hex_session::arrangement_seek::SectionLaunch {
        region,
        quantization: hex_engine::LaunchQuantization::Immediate,
    });
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    assert!(h.seek_result().result.is_ok()); // Submitted, deliberately not rendered/applied.
    h.service
        .submit(section_request(
            event.state.revision,
            Command::RenameArrangementSection {
                section: intro,
                name: "pending must refuse".into(),
            },
        ))
        .unwrap();
    let refused = h.next();
    assert!(
        refused
            .error
            .as_ref()
            .is_some_and(|e| e.contains("publication already pending"))
    );
    assert_eq!(refused.state.revision, event.state.revision);
    assert_eq!(refused.state.project.snapshot(), before);
    (applied, _) = h.applied(applied);
    let renamed = h.run(section_request(
        event.state.revision,
        Command::RenameArrangementSection {
            section: intro,
            name: "applied is editable".into(),
        },
    ));
    (applied, _) = h.applied(applied);
    assert_loop_wraps(&h, region);

    let mut intent = h.seek_intent(3, renamed.state.revision, None, None);
    // A distinct ordinary seek can be cancelled while the canonical section
    // loop remains enabled; a different enabled section loop is no longer a
    // valid launch unless the project loop changes first.
    intent.destination_frame = 20_000;
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    let hex_session::service::ResidentPublicationAcceptance::Submitted {
        audio_revision: cancelled_revision,
        ..
    } = h.seek_result().result.unwrap()
    else {
        panic!("active output must submit the conditional launch")
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    // The queued Stop settles after its declick fade.
    for _ in 0..super::FADE_BLOCKS {
        h.render();
    }
    assert!(h.render().iter().flatten().all(|s| *s == 0.0));
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(!status.playing.load(Acquire));
    assert_eq!(status.rejected_revision().unwrap().0, cancelled_revision);
    assert_eq!(status.applied_revision.load(Acquire), applied);
    let stopped_position = status.position.load(Acquire);
    h.run(section_request(
        renamed.state.revision,
        Command::RenameArrangementSection {
            section: intro,
            name: "after cancellation".into(),
        },
    ));
    h.applied(cancelled_revision);
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(!status.playing.load(Acquire));
    assert_eq!(status.position.load(Acquire), stopped_position);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..400 {
        h.render();
    }
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire),
        (stopped_position + 400 * 64) % region.end(),
        "the cancelled seek must not replace the actual section loop"
    );
}

#[test]
fn new_document_and_output_do_not_inherit_the_previous_audition_loop() {
    let _native = native_fixture();
    let MetadataLoop { mut h, region, .. } = metadata_loop_fixture();
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let new = h.run(Request::NewWithAudioMode {
        mode: hex_session::DocumentAudioMode::Resident,
        reservation: None,
    });
    assert!(new.project_replaced);
    let applied = super::resident_lifecycle::finish_replacement(&mut h, &new, old_output);
    let added = h.run(Request::EditArrangementSection {
        project_generation: 1,
        revision: new.state.revision,
        command: Command::AddArrangementSection {
            name: "fresh".into(),
            color: ElementColor::Cyan,
            start: BeatTime::ZERO,
            end: BeatTime::from_quarters(7).unwrap(),
        },
    });
    assert!(added.state.project.loop_region().is_none());
    h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
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
            >= region.end(),
        "new document must play beyond the previous audition loop"
    );
}

/// Stopped section edits and their history start device tails empty at the raw
/// cursor instead of replaying the song prefix through stateful devices.
#[test]
fn stopped_section_edits_reset_device_tails_at_a_late_cursor_without_replay() {
    const QUARTER: u64 = 24_000; // 120 bpm at 48 khz.
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(8_000_000);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, QUARTER as u32)));
    let track = imported.state.project.tracks()[0].id();
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    (applied, _) = h.applied(applied);
    h.run(Request::Edit(Command::MoveClip {
        clip,
        track,
        start: 200 * QUARTER,
    }));
    (applied, _) = h.applied(applied);
    let added = h.run(Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.reverb")),
        before: None,
    });
    assert!(added.error.is_none(), "{:?}", added.error);
    (applied, _) = h.applied(applied);
    let (event, section) = add(&mut h, added.state.revision, "hit", 200, 201);
    (applied, _) = h.applied(applied);
    let seek = |h: &mut ResidentHarness, frame: u64| {
        let audio = h.audio.as_mut().unwrap();
        audio.control.command(PlaybackCommand::Pause).unwrap();
        audio.control.command(PlaybackCommand::Seek(frame)).unwrap();
        h.render();
    };
    let position = |h: &ResidentHarness| {
        let status = h.audio.as_ref().unwrap().status().unwrap();
        (status.position.load(Acquire), status.playing.load(Acquire))
    };
    // Silence after both the original (ends at 201) and the duplicate (ends at 202).
    let cursor = 202 * QUARTER + 2_400;
    seek(&mut h, cursor);
    let play_energy = |h: &mut ResidentHarness| {
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
        let audio = h.audio.as_mut().unwrap();
        audio.control.command(PlaybackCommand::Play).unwrap();
        (0..8)
            .flat_map(|_| h.render())
            .flatten()
            .map(|v| f64::from(v).powi(2))
            .sum::<f64>()
    };
    let duplicated = run_callback(
        &mut h,
        section_request(
            event.state.revision,
            Command::DuplicateArrangementSection(section),
        ),
    );
    assert!(duplicated.error.is_none(), "{:?}", duplicated.error);
    assert_eq!(duplicated.state.project.arrangement_sections().len(), 2);
    for step in [None, Some(Request::Undo), Some(Request::Redo)] {
        if let Some(request) = step {
            seek(&mut h, cursor);
            let event = run_callback(&mut h, request);
            assert!(event.error.is_none(), "{:?}", event.error);
        }
        (applied, _) = h.applied(applied);
        assert_eq!(position(&h), (cursor, false));
        // An exact prefix replay would leave the hit's reverb decay sounding here.
        assert_eq!(play_energy(&mut h), 0.0, "device tails start empty");
    }
    // Playback renders the edited arrangement: the duplicate now sounds at 201.
    seek(&mut h, 201 * QUARTER + 2_400);
    assert!(play_energy(&mut h) > 1e-6, "duplicated audio plays");
    // Every h.render above checks both total and signed-net allocations/bytes.
}

/// Section requests report their worker phases on the `OutcomeReady`
/// observation (#3819): marker-only edits have no callback fence, structural
/// edits and their history wait for one.
#[test]
fn section_outcomes_report_worker_phases() {
    use hex_session::service::diagnostics::{DiagnosticObserver, DiagnosticStage};
    let _native = native_fixture();
    let mut h = ResidentHarness::new();
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&outcomes);
    h.service
        .set_diagnostic_observer(Arc::new(DiagnosticObserver::new(move |event| {
            if matches!(event.stage, DiagnosticStage::OutcomeReady { .. }) {
                sink.lock().unwrap().push(event.details.clone());
            }
        })))
        .unwrap();
    let initial = h.next();
    assert!(initial.error.is_none());
    let (applied, _) = h.applied(0);
    let phases = |details: &str, name: &str| -> u64 {
        details
            .split_whitespace()
            .find_map(|field| field.strip_prefix(name)?.strip_prefix('=')?.parse().ok())
            .unwrap_or_else(|| panic!("{name} missing from {details:?}"))
    };
    outcomes.lock().unwrap().clear();
    let (added, intro) = add(&mut h, initial.state.revision, "intro", 0, 4);
    h.applied(applied);
    let details = outcomes.lock().unwrap().last().cloned().unwrap();
    assert_eq!(phases(&details, "section_fence_us"), 0, "marker-only edit");
    assert!(phases(&details, "worker_us") >= phases(&details, "section_prepare_us"));
    for request in [
        section_request(
            added.state.revision,
            Command::DuplicateArrangementSection(intro),
        ),
        Request::Undo,
    ] {
        outcomes.lock().unwrap().clear();
        let event = run_callback(&mut h, request);
        assert!(event.error.is_none(), "{:?}", event.error);
        let details = outcomes.lock().unwrap().last().cloned().unwrap();
        let worker = phases(&details, "worker_us");
        let measured: u64 = [
            "section_fence_us",
            "section_edit_us",
            "section_prepare_us",
            "section_publish_us",
            "section_finish_us",
        ]
        .iter()
        .map(|name| phases(&details, name))
        .sum();
        assert!(
            measured <= worker,
            "phases {measured} within worker {worker}"
        );
    }
}
#[test]
fn marker_only_edits_and_history_republish_without_preparation() {
    use hex_session::service::diagnostics::{DiagnosticObserver, DiagnosticStage};
    let _native = native_fixture();
    let mut h = ResidentHarness::new();
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&outcomes);
    h.service
        .set_diagnostic_observer(Arc::new(DiagnosticObserver::new(move |event| {
            if matches!(event.stage, DiagnosticStage::OutcomeReady { .. }) {
                sink.lock().unwrap().push(event.details.clone());
            }
        })))
        .unwrap();
    h.next();
    let (mut applied, _) = h.applied(0);
    let track = h.run(Request::Edit(Command::AddMidiTrack {
        name: "song".into(),
    }));
    (applied, _) = h.applied(applied);
    let Some(Created::Track(track)) = track.created else {
        panic!("track");
    };
    let clip = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8).unwrap(),
    }));
    (applied, _) = h.applied(applied);
    let tracks = clip.state.project.tracks().to_vec();
    let prepare_us = |outcomes: &Mutex<Vec<String>>| -> u64 {
        let details = outcomes.lock().unwrap().last().cloned().unwrap();
        details
            .split_whitespace()
            .find_map(|f| f.strip_prefix("section_prepare_us=")?.parse().ok())
            .unwrap_or_else(|| panic!("section_prepare_us missing from {details:?}"))
    };
    outcomes.lock().unwrap().clear();
    let (added, intro) = add(&mut h, clip.state.revision, "intro", 0, 4);
    assert_eq!(prepare_us(&outcomes), 0, "add marker");
    assert_eq!(added.state.revision, clip.state.revision + 1);
    // Every marker edit still publishes, so the native context follows the
    // revision; `applied` waits for the callback to install each one.
    (applied, _) = h.applied(applied);
    let mut states = Vec::new();
    let mut revision = added.state.revision;
    for command in [
        Command::MoveArrangementSectionMarker {
            section: intro,
            start: BeatTime::from_quarters(1).unwrap(),
        },
        Command::RenameArrangementSection {
            section: intro,
            name: "verse".into(),
        },
    ] {
        outcomes.lock().unwrap().clear();
        let event = h.run(section_request(revision, command));
        assert_eq!(prepare_us(&outcomes), 0);
        assert_eq!(event.state.revision, revision + 1);
        assert_eq!(event.state.project.tracks(), tracks);
        (applied, _) = h.applied(applied);
        revision = event.state.revision;
        states.push(Arc::clone(&event.state.project));
    }
    // A structural edit after marker-only republication takes its fenced lease.
    outcomes.lock().unwrap().clear();
    let duplicated = run_callback(
        &mut h,
        section_request(revision, Command::DuplicateArrangementSection(intro)),
    );
    assert!(duplicated.error.is_none(), "{:?}", duplicated.error);
    assert!(prepare_us(&outcomes) > 0, "structural edits still prepare");
    (applied, _) = h.applied(applied);
    // History keeps the ID allocator advanced; compare everything else.
    let same = |actual: &hex_project::Project, expected: &hex_project::Project| {
        let mut actual = actual.snapshot();
        actual.next_id = expected.snapshot().next_id;
        assert_eq!(actual, expected.snapshot());
    };
    let undone = run_callback(&mut h, Request::Undo);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    same(&undone.state.project, &states[1]);
    (applied, _) = h.applied(applied);
    // Undo/Redo of the rename republish without preparation and restore exactly.
    for (request, expected) in [(Request::Undo, &states[0]), (Request::Redo, &states[1])] {
        outcomes.lock().unwrap().clear();
        let event = run_callback(&mut h, request);
        assert!(event.error.is_none(), "{:?}", event.error);
        assert_eq!(prepare_us(&outcomes), 0);
        same(&event.state.project, expected);
        (applied, _) = h.applied(applied);
    }
}
