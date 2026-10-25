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
