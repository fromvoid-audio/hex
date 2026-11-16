use super::*;
use hex_lattice_contract::NativeDeviceMetadata;

fn start_fixture(start: u64, duration: u64, limiter: Option<bool>) -> Arc<PreparedRevision> {
    let Some(main) = limiter else {
        return mutable_checkpoint_fixture_at(start, duration, 8).0;
    };
    let definition = super::source_loop_tests::mutable_definition();
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let project = with_test_instrument(&project(1.0, start, duration), definition.metadata.id, 0);
    let mut snapshot = project.snapshot();
    let device = DeviceSnapshot {
        modulation: Default::default(),
        parent: None,
        automation: Vec::new(),
        comparison: None,
        id: snapshot.next_id,
        bypassed: false,
        state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
    };
    snapshot.next_id += 1;
    if main {
        snapshot.main_devices.push(device);
    } else {
        snapshot.tracks[0].devices.push(device);
    }
    revision_with_instrument_registry(
        &Project::from_snapshot(snapshot).unwrap(),
        Arc::new(hex_devices::SineTable::new()),
        &registry,
    )
}

#[test]
#[ignore = "optimized timing evidence; run with --release and --ignored"]
fn profile_interactive_start_against_song_prefix_preparation() -> Result<(), &'static str> {
    if cfg!(debug_assertions) {
        return Err("use --release for timing");
    }
    std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
        let prepared = mutable_hundred_track_preroll_revision();
        let mut runtime = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        let mut output = [[0.0; 2]; 256];
        for destination in [0, 24_000, 144_000] {
            let mut old = Vec::new();
            let mut new = Vec::new();
            for iteration in 0..4 {
                let start = std::time::Instant::now();
                let mut candidate = SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                    Arc::clone(&prepared), destination, destination, true, &AtomicBool::new(false),
                ).unwrap();
                candidate.render(&mut output).unwrap();
                let elapsed = start.elapsed();
                if iteration != 0 { old.push(elapsed); }
                let start = std::time::Instant::now();
                runtime.play_from(destination).unwrap();
                runtime.render(&mut output).unwrap();
                let elapsed = start.elapsed();
                if iteration != 0 { new.push(elapsed); }
            }
            eprintln!("interactive-start tracks=100 mutable-feedback-fixture rate=48000 block=256 destination={destination} warmup=1 samples=3 prefix-preparation-plus-first-render={old:?} reset-and-chase-plus-first-render={new:?}; excludes UI/OS/device and does not establish physical deadlines");
        }
    }).unwrap().join().unwrap();
    Ok(())
}

#[test]
fn chased_mutable_note_keeps_pcm_and_note_off_through_an_ordinary_handoff() {
    let (prepared, _) = mutable_checkpoint_fixture(1 << 32);
    let mut source = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut oracle = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    for runtime in [&mut source, &mut oracle] {
        runtime.play_from(12_000).unwrap();
        runtime.render(&mut [[0.0; 2]; 127]).unwrap();
    }
    let mut successor = SessionPlayback::new(prepared).unwrap();
    successor.try_inherit_state_from(&mut source).unwrap();
    let expected = render_checkpoint_partitioned(&mut oracle, &[256], 12_064);
    let actual = render_checkpoint_partitioned(&mut successor, &[1, 7, 193], 12_064);
    assert_eq!(actual, expected);
    assert!(
        actual[12_040..]
            .iter()
            .flatten()
            .all(|sample| *sample == 0.0)
    );
}

#[test]
fn play_from_restarts_a_held_session_note_across_render_partitions() {
    // At 120 bpm and 48 kHz this gate lasts 1,500 frames; the fixture's
    // release and feedback tail last at most 16 more. The first instance is
    // still held at the cut.
    let (prepared, _) = mutable_checkpoint_fixture(1 << 28);
    let clip = prepared.session().midi_clips()[0].id;
    let launch = |runtime: &mut SessionPlayback| {
        runtime
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
    };

    // A fresh literal launch at the destination is independent of the old
    // voice and gives the expected onset, gate and release PCM.
    let mut literal = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    literal.apply(SessionPlaybackCommand::Seek(12_000)).unwrap();
    launch(&mut literal);
    literal.apply(SessionPlaybackCommand::Play).unwrap();
    let expected = render_checkpoint_partitioned(&mut literal, &[512], 1_520);
    assert!(
        expected[..1_500]
            .iter()
            .flatten()
            .any(|sample| *sample != 0.0)
    );
    assert!(
        expected[1_518..]
            .iter()
            .flatten()
            .all(|sample| *sample == 0.0)
    );

    for block_size in [1, 7, 64, 128, 256, 512] {
        let mut actual = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        launch(&mut actual);
        actual.apply(SessionPlaybackCommand::Play).unwrap();
        let held = render_checkpoint_partitioned(&mut actual, &[128], 100);
        assert!(held.iter().flatten().any(|sample| *sample != 0.0));
        actual.play_from(12_000).unwrap();
        let rendered = render_checkpoint_partitioned(&mut actual, &[block_size], 1_520);
        assert_eq!(rendered, expected, "block size {block_size}");
    }
}

#[test]
fn repeated_play_from_invalidates_a_projected_stream_action_at_the_same_clock() {
    let (prepared, _) = mutable_checkpoint_fixture(1 << 32);
    let mut runtime = SessionPlayback::new(prepared).unwrap();
    runtime.play_from(12_000).unwrap();
    let plan = runtime
        .prepare_stream_action_in_scope(
            PlaybackAction::Transport(PlaybackCommand::PlayFrom(12_000)),
            true,
        )
        .unwrap();
    runtime.play_from(12_000).unwrap();
    assert_eq!(
        runtime.validate_stream_action(&plan),
        Err(SessionPlaybackError::InvalidCommand)
    );
}

#[test]
fn play_from_mutable_cursor_before_inside_and_after_loop_keeps_absolute_transport() {
    let project = super::source_loop_tests::arrangement_loop_project();
    let region = LoopRegion::new(48_000, 96_000, true).unwrap();
    let mut snapshot = project.snapshot();
    snapshot.loop_region = Some(region);
    let prepared =
        super::source_loop_tests::prepare_mutable(&Project::from_snapshot(snapshot).unwrap());
    for destination in [0, 47_999, 48_000, 72_000, 95_999, 96_000, 120_000] {
        let mut runtime = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        runtime.play_from(destination).unwrap();
        assert_eq!(runtime.arrangement_frame(), destination);
        let _ = render_checkpoint_partitioned(&mut runtime, &[1, 7, 256], 257);
        let expected = if destination < 96_000 && destination + 257 >= 96_000 {
            48_000 + destination + 257 - 96_000
        } else {
            destination + 257
        };
        assert_eq!(runtime.arrangement_frame(), expected);
        assert_eq!(runtime.performance_frame(), destination + 257);
        assert_eq!(runtime.loop_region(), Some(region));
    }
}

#[test]
fn play_from_chases_a_held_mutable_note_without_replaying_the_prefix() {
    // Exercise ordinary chunks, upstream per-frame compensation, and the
    // Main-only batching path. Each must retain the same fresh onset law.
    for limiter in [None, Some(false), Some(true)] {
        let prepared = start_fixture(0, 1 << 32, limiter);
        let latency = prepared.arrangement().main_latency_frames() as usize;
        let mut playback = Playback::try_from_prepared_with_session(prepared).unwrap();
        // Compare against independently authored music whose note starts at the
        // cursor and ends at the original gate. Prefix replay would leave a
        // different feedback state; lost or extended note-offs also change PCM.
        let oracle = start_fixture(1 << 31, 1 << 31, limiter);
        let mut oracle = SessionPlayback::new(oracle).unwrap();
        oracle.apply(SessionPlaybackCommand::Play).unwrap();
        let prefix = render_checkpoint_prefix(&mut oracle, 12_000);
        assert!(prefix.iter().flatten().all(|value| *value == 0.0));
        let expected = render_checkpoint_partitioned(&mut oracle, &[256], 12_064 + latency);
        playback
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(12_000)))
            .unwrap();
        assert_eq!(playback.position(), 12_000);
        assert!(playback.playing());
        let mut actual = Vec::new();
        for count in [0, 1, 7, 256, 193, 11_607 + latency] {
            let mut block = vec![[0.0; 2]; count];
            playback.render(&mut block).unwrap();
            actual.extend(block);
        }
        assert_eq!(actual, expected, "limiter location={limiter:?}");
        assert!(actual[..12_000].iter().flatten().any(|value| *value != 0.0));
        assert!(
            actual[12_040 + latency..]
                .iter()
                .flatten()
                .all(|value| *value == 0.0)
        );
    }
}

#[test]
fn short_session_note_refusal_reports_recovery_and_preserves_project() {
    let one_frame = (1_u64 << 32).div_ceil(24_000);
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let project = with_test_instrument(&project(1.0, 0, one_frame), id, 0);
    let before = project.snapshot();
    let Err(error) = PreparedRevision::prepare_with_instrument_registry(
        &project,
        &BTreeMap::new(),
        Arc::new(hex_devices::SineTable::new()),
        None,
        &registry,
    ) else {
        panic!("short Session source must be refused");
    };
    assert_eq!(
        error.0,
        "session midi note gate is too short to guarantee one frame at the admitted tempo; lengthen the note or reduce tempo"
    );
    assert_eq!(project.snapshot(), before);

    let mut short_loop = project.snapshot();
    let hex_project::SessionCellContentSnapshot::Midi(clip) =
        &mut short_loop.session.cells[0].content
    else {
        panic!("session midi fixture");
    };
    clip.source.looping = true;
    clip.source.loop_end_raw = one_frame;
    // Use a long gate so loop-period admission is the first refusal.
    clip.source.notes[0].duration_raw = 1 << 32;
    let short_loop = Project::from_snapshot(short_loop).unwrap();
    let before_loop = short_loop.snapshot();
    let Err(error) = PreparedRevision::prepare_with_instrument_registry(
        &short_loop,
        &BTreeMap::new(),
        Arc::new(hex_devices::SineTable::new()),
        None,
        &registry,
    ) else {
        panic!("short Session loop must be refused");
    };
    assert_eq!(
        error.0,
        "session midi loop is too short to guarantee one frame at the admitted tempo; lengthen the loop or reduce tempo"
    );
    assert_eq!(short_loop.snapshot(), before_loop);
}

#[test]
fn play_from_one_frame_before_note_off_matches_one_frame_authored_note() {
    // At 120 bpm, a quarter note ends at frame 24,000. Round the independent
    // one-frame beat duration up so its note-off lands on frame one.
    let one_frame = (1_u64 << 32).div_ceil(24_000);
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let mut snapshot = project(1.0, 0, one_frame).snapshot();
    // The oracle exercises Arrangement; omit the unrelated Session source so
    // its own template admission does not constrain this short-note fixture.
    snapshot.session.cells.clear();
    let oracle_project = with_test_instrument(&Project::from_snapshot(snapshot).unwrap(), id, 0);
    let oracle_revision = revision_with_instrument_registry(
        &oracle_project,
        Arc::new(hex_devices::SineTable::new()),
        &registry,
    );
    let mut oracle = SessionPlayback::new(oracle_revision).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let expected = render_checkpoint_partitioned(&mut oracle, &[64], 512);
    assert!(expected[0].iter().any(|sample| *sample != 0.0));
    assert!(expected[1..8].iter().flatten().any(|sample| *sample != 0.0));
    assert!(expected[16..].iter().flatten().all(|sample| *sample == 0.0));

    let prepared = start_fixture(0, 1 << 32, None);
    for block in [1, 7, 17, 64, 128, 256, 512] {
        let mut playback = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        playback.play_from(23_999).unwrap();
        let actual = render_checkpoint_partitioned(&mut playback, &[block], 512);
        assert_eq!(actual, expected, "block size {block}");
        assert_eq!(playback.arrangement_frame(), 24_511);
        assert_eq!(playback.performance_frame(), 24_511);
    }
}

#[test]
fn play_from_note_onset_and_off_boundaries_match_authored_pcm() {
    // At 120 bpm and 48 kHz this one-beat note ends at frame 24,000.
    for limiter in [None, Some(false), Some(true)] {
        let prepared = start_fixture(0, 1 << 32, limiter);
        let latency = prepared.arrangement().main_latency_frames() as usize;
        assert_eq!(latency > 0, limiter.is_some());
        let frames = 512 + latency;
        let mut oracle = SessionPlayback::new(start_fixture(0, 1 << 32, limiter)).unwrap();
        oracle.apply(SessionPlaybackCommand::Play).unwrap();
        let onset = render_checkpoint_partitioned(&mut oracle, &[256], frames);
        assert!(onset.iter().flatten().any(|sample| *sample != 0.0));

        for destination in [0, 24_000, 24_001] {
            let expected = if destination == 0 {
                onset.clone()
            } else {
                vec![[0.0; 2]; frames]
            };
            for block in [1, 17, 64, 128, 256, 512] {
                let mut playback = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
                playback.play_from(destination).unwrap();
                let actual = render_checkpoint_partitioned(&mut playback, &[block], frames);
                assert_eq!(
                    actual, expected,
                    "limiter={limiter:?}, destination={destination}, block={block}"
                );
                assert_eq!(playback.arrangement_frame(), destination + frames as u64);
                assert_eq!(playback.performance_frame(), destination + frames as u64);
            }
        }
    }
}

#[test]
fn chased_note_gate_and_stop_match_authored_pcm_at_fixed_block_sizes() {
    let prepared = start_fixture(0, 1 << 32, None);
    let mut oracle = SessionPlayback::new(start_fixture(1 << 31, 1 << 31, None)).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    assert!(
        render_checkpoint_prefix(&mut oracle, 12_000)
            .iter()
            .flatten()
            .all(|value| *value == 0.0)
    );
    let expected = render_checkpoint_partitioned(&mut oracle, &[256], 12_064);
    assert!(
        expected[..12_000]
            .iter()
            .flatten()
            .any(|value| *value != 0.0)
    );
    assert!(
        expected[12_040..]
            .iter()
            .flatten()
            .all(|value| *value == 0.0)
    );
    for block in [1, 17, 64, 128, 256, 512] {
        let mut playback = Playback::try_from_prepared_with_session(Arc::clone(&prepared)).unwrap();
        playback
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(12_000)))
            .unwrap();
        let mut actual = vec![[0.0; 2]; expected.len()];
        for chunk in actual.chunks_mut(block) {
            playback.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "block size {block}");
        playback
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Stop))
            .unwrap();
        let mut stopped = [[1.0; 2]; 32];
        playback.render(&mut stopped).unwrap();
        assert_eq!(stopped, [[0.0; 2]; 32]);
        assert_eq!(playback.position(), 0);
    }
}

#[test]
fn stateful_seek_candidate_chases_held_note_with_empty_tails_at_fixed_block_sizes() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            for limiter in [None, Some(false), Some(true)] {
                let prepared = start_fixture(0, 1 << 32, limiter);
                assert!(prepared.arrangement_seek_requires_preroll());
                let latency = prepared.arrangement().main_latency_frames() as usize;
                // Independently authored music whose note starts at the seek
                // point and ends at the original gate; any replayed feedback
                // or limiter history would change the PCM.
                let mut oracle =
                    SessionPlayback::new(start_fixture(1 << 31, 1 << 31, limiter)).unwrap();
                oracle.apply(SessionPlaybackCommand::Play).unwrap();
                let _ = render_checkpoint_prefix(&mut oracle, 12_000);
                let expected = render_checkpoint_partitioned(&mut oracle, &[256], 12_064 + latency);
                for playing in [true, false] {
                    for block in [1, 64, 256, 512] {
                        let candidate =
                            SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                                Arc::clone(&prepared),
                                12_000,
                                12_000,
                                playing,
                                &AtomicBool::new(false),
                            )
                            .unwrap();
                        let mut playback =
                            Playback::from_interactive_arrangement_preroll(candidate).unwrap();
                        assert_eq!(playback.position(), 12_000);
                        assert_eq!(playback.playing(), playing);
                        if !playing {
                            let mut frozen = [[1.0; 2]; 19];
                            playback.render(&mut frozen).unwrap();
                            assert_eq!(frozen, [[0.0; 2]; 19]);
                            assert_eq!(playback.position(), 12_000);
                            playback.apply(PlaybackCommand::Play);
                        }
                        let mut actual = vec![[0.0; 2]; expected.len()];
                        for chunk in actual.chunks_mut(block) {
                            playback.render(chunk).unwrap();
                        }
                        assert_eq!(
                            actual, expected,
                            "limiter={limiter:?} playing={playing} block={block}"
                        );
                    }
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn play_from_invalid_destination_is_atomic_and_strict_seek_still_refuses_chase() {
    let (prepared, track) = mutable_checkpoint_fixture(1 << 32);
    let mut runtime = SessionPlayback::new(prepared).unwrap();
    assert_eq!(
        runtime.apply(SessionPlaybackCommand::Seek(12_000)),
        Err(SessionPlaybackError::MutableInstrumentChase)
    );
    runtime.play_from(12_000).unwrap();
    let before = (
        runtime.performance_frame(),
        runtime.route_state_generation(),
    );
    assert!(runtime.play_from(u64::MAX).is_err());
    assert_eq!(
        (
            runtime.performance_frame(),
            runtime.route_state_generation()
        ),
        before
    );
    assert!(runtime.playing());
    let plan = runtime
        .output_capture_plan(&[OutputCaptureId::Track(track)], 1)
        .unwrap();
    let mut main = [[0.0; 2]; 2];
    let mut tapped = main;
    let mut buffers: [&mut [Frame]; 1] = [&mut tapped];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    assert!(
        runtime
            .render_with_capture(&mut main, &mut capture)
            .is_err()
    );
    assert_eq!(runtime.performance_frame(), 12_000);
    let mut actual = [[0.0; 2]; 32];
    runtime.render(&mut actual).unwrap();
    assert!(actual.iter().flatten().any(|v| *v != 0.0));
}

#[test]
fn play_from_resets_active_mutable_state_and_repeated_starts_are_identical() {
    let (prepared, _) = mutable_checkpoint_fixture(1 << 32);
    let mut playback = Playback::try_from_prepared_with_session(prepared).unwrap();
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    playback.render(&mut [[0.0; 2]; 256]).unwrap();
    let mut previous = None;
    for _ in 0..3 {
        playback
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(12_000)))
            .unwrap();
        let mut output = [[0.0; 2]; 193];
        playback.render(&mut output).unwrap();
        assert!(output.iter().flatten().any(|value| *value != 0.0));
        if let Some(expected) = previous {
            assert_eq!(output, expected);
        }
        previous = Some(output);
    }
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Stop))
        .unwrap();
    let mut output = [[1.0; 2]; 32];
    playback.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 32]);
}

#[test]
fn shared_chase_and_handoff_without_limiter() {
    shared_chase_and_handoff(None, &[0, 127]);
}

#[test]
fn shared_chase_immediate_handoff_with_track_limiter() {
    shared_chase_and_handoff(Some(false), &[0]);
}

#[test]
fn shared_chase_delayed_handoff_with_track_limiter() {
    shared_chase_and_handoff(Some(false), &[127]);
}

#[test]
fn shared_chase_and_handoff_with_main_limiter() {
    shared_chase_and_handoff(Some(true), &[0, 127]);
}

fn shared_chase_and_handoff(limiter: Option<bool>, handoff_frames: &[usize]) {
    fn shared(start: u64, duration: u64, limiter: Option<bool>, pitch: u8) -> Project {
        let definition = super::source_loop_tests::mutable_definition();
        let project =
            with_test_instrument(&project(1.0, start, duration), definition.metadata.id, 0);
        let receiver = project.tracks()[0].id();
        let source = project.track(receiver).unwrap().source_id().unwrap();
        let mut editor = Editor::new(project, 8).unwrap();
        let Created::Track(contributor) = editor
            .execute(Command::AddMidiTrack {
                name: "contributor".into(),
            })
            .unwrap()
        else {
            panic!("contributor")
        };
        editor
            .execute(Command::BindTrackSource {
                track: contributor,
                source: Some(source),
            })
            .unwrap();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track: contributor,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::from_raw(start),
                duration: BeatTime::from_raw(duration),
                pitch,
                velocity: 100,
                channel: 0,
            })
            .unwrap();
        let mut snapshot = editor.project().snapshot();
        if let Some(main) = limiter {
            let device = DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: snapshot.next_id,
                bypassed: false,
                state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
            };
            snapshot.next_id += 1;
            if main {
                snapshot.main_devices.push(device);
            } else {
                snapshot.tracks[0].devices.push(device);
            }
        }
        Project::from_snapshot(snapshot).unwrap()
    }
    let definition = super::source_loop_tests::mutable_definition();
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    for &first in handoff_frames {
        let project = shared(0, 1 << 32, limiter, 67);
        let mut runtime = ordinary_mutable_playback(&project, &registry);
        let latency = runtime.revision.arrangement().main_latency_frames() as usize;
        let mut oracle =
            ordinary_mutable_playback(&shared(1 << 31, 1 << 31, limiter, 67), &registry);
        oracle.apply(SessionPlaybackCommand::Play).unwrap();
        assert!(
            render_checkpoint_prefix(&mut oracle, 12_000)
                .iter()
                .flatten()
                .all(|s| *s == 0.0)
        );
        runtime.play_from(12_000).unwrap();
        if first == 0 {
            let mut changed =
                ordinary_mutable_playback(&shared(0, 1 << 32, limiter, 68), &registry);
            assert_eq!(
                changed.try_inherit_state_from(&mut runtime),
                Err(SessionPlaybackError::MutableInstrumentChase)
            );
            assert_eq!(runtime.performance_frame(), 12_000);
            assert_eq!(runtime.interactive_start_frame, Some(12_000));
        }
        let expected = render_checkpoint_partitioned(&mut oracle, &[256], 12_064 + latency);
        let mut actual = render_checkpoint_partitioned(&mut runtime, &[1, 7, 193], first);
        let mut successor = ordinary_mutable_playback(&project, &registry);
        successor
            .preview_stream_handoff_with_scope(&mut runtime, 32, None, &mut |_| Ok(()), true)
            .unwrap_or_else(|error| panic!("preview limiter={limiter:?} first={first}: {error:?}"));
        successor.try_inherit_state_from(&mut runtime).unwrap();
        actual.extend(render_checkpoint_partitioned(
            &mut successor,
            &[63, 257],
            12_064 + latency - first,
        ));
        assert_eq!(actual, expected, "limiter={limiter:?}");
        assert!(actual[..12_000].iter().flatten().any(|s| *s != 0.0));
        assert!(
            actual[12_040 + latency..]
                .iter()
                .flatten()
                .all(|s| *s == 0.0)
        );
    }
}
