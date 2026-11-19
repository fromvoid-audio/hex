//! Check the actual Session tempo edit against the hosted worker's PCM.
use super::{instruments::render_completed_block, *};
use hex_engine::playback::PlaybackCommand;
use hex_project::Tempo;

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn running_tempo_edit_preserves_hosted_pcm_and_beat() {
    let _native = native_fixture();
    for streamed in [false, true] {
        run(streamed);
    }
}

fn start_playback(files: &Files, streamed: bool, looped: bool) -> (ResidentHarness, Event, u64) {
    let frames = if streamed { 96_000 } else { 8192 };
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (initial, _) = h.applied(0);
    let mut imported = h.run(Request::Import(files.wav(48_000, frames)));
    let (mut imported_revision, _) = h.applied(initial);
    let track = imported.state.project.tracks()[0].id();
    assert!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .required_sources()
            .all(|(_, source)| {
                matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)) == streamed
            })
    );
    if looped {
        imported = h.run(Request::Edit(Command::SetLoopRegion {
            start: 768,
            end: 2304,
            enabled: true,
        }));
        (imported_revision, _) = h.applied(imported_revision);
    }
    h.service
        .submit(Request::InsertPluginEffect(request(
            &imported.state,
            PluginEffectDestination::Track(track),
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(accepted.plugin_effect_complete[0].accepted);
    let (applied, _) = h.applied(imported_revision);

    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    (h, accepted, applied)
}

fn run(streamed: bool) {
    let files = Files::new();
    let (mut h, accepted, applied) = start_playback(&files, streamed, false);
    let original_map = accepted.state.project.tempo_map().clone();
    let mut ordinal = 0;
    let ready = Instant::now() + Duration::from_secs(2);
    while ordinal < 6 {
        let block = render_completed_block(&h);
        let status = h.audio.as_ref().unwrap().status().unwrap();
        if status.stream_buffering.load(Acquire) {
            assert_eq!(block, [[0.0; 2]; 256]);
            assert_eq!(status.performance_frame.load(Acquire), ordinal as u64 * 256);
            assert!(
                Instant::now() < ready,
                "initial audio pages did not become ready"
            );
            continue;
        }
        assert_pcm(ordinal, &block);
        ordinal += 1;
    }

    h.service
        .submit(Request::Edit(Command::SetProjectTempo {
            tempo: Tempo::new(90_000_000).unwrap(),
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    let mut edited = None;
    let mut cut = None;
    while edited.is_none() || cut.is_none() {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            if event.state.revision > accepted.state.revision {
                edited = Some(event);
            }
        }
        let status = h.audio.as_ref().unwrap().status().unwrap();
        let previous_revision = status.applied_revision.load(Acquire);
        let previous_position = status.position.load(Acquire);
        let pcm = render_completed_block(&h);
        if previous_revision == applied && status.applied_revision.load(Acquire) > applied {
            cut = Some((previous_position, status.position.load(Acquire)));
        }
        assert!(
            !status.stream_buffering.load(Acquire),
            "tempo edit interrupted the previous playback at frame {previous_position}"
        );
        assert_pcm(ordinal, &pcm);
        ordinal += 1;
        assert!(
            Instant::now() < until,
            "tempo edit did not reach the callback"
        );
    }
    let (before, after) = cut.unwrap();
    let beat = original_map.frame_to_beat(before).unwrap();
    let relocated = edited
        .unwrap()
        .state
        .project
        .tempo_map()
        .beat_to_frame(beat)
        .unwrap();
    assert_eq!(after, relocated + 256);
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Acquire) && status.rejected_revision().is_none());
    for _ in 0..3 {
        assert_pcm(ordinal, &render_completed_block(&h));
        ordinal += 1;
    }
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn running_loop_tempo_edit_keeps_hosted_pcm_after_a_wrap() {
    let _native = native_fixture();
    for streamed in [false, true] {
        let render_from_start = |h: &ResidentHarness, ordinal: u64| {
            let until = Instant::now() + Duration::from_secs(2);
            loop {
                let pcm = render_completed_block(h);
                let status = h.audio.as_ref().unwrap().status().unwrap();
                if !status.stream_buffering.load(Acquire) {
                    assert_eq!(status.performance_frame.load(Acquire), (ordinal + 1) * 256);
                    return pcm;
                }
                assert_eq!(pcm, [[0.0; 2]; 256]);
                assert_eq!(status.performance_frame.load(Acquire), ordinal * 256);
                assert!(
                    Instant::now() < until,
                    "initial loop pages did not become ready"
                );
            }
        };
        // Capture the uninterrupted oracle first. The native source holds
        // the exclusive output lease for the lifetime of the harness.
        let expected: Vec<_> = {
            let files = Files::new();
            let (uninterrupted, _, _) = start_playback(&files, streamed, true);
            (0..128)
                .map(|ordinal| render_from_start(&uninterrupted, ordinal))
                .collect()
        };
        let files = Files::new();
        let (mut playback, applied, revision) = start_playback(&files, streamed, true);
        for (ordinal, expected) in expected.iter().take(12).enumerate() {
            let pcm = render_from_start(&playback, ordinal as u64);
            if ordinal == 2 {
                assert_pcm(2, &pcm);
            }
            assert_eq!(&pcm, expected, "loop warm-up block {ordinal}");
        }
        assert_eq!(
            playback
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .position
                .load(Acquire),
            1536
        );
        let old_tempo = applied.state.project.tempo_map();
        playback
            .service
            .submit(Request::Edit(Command::SetProjectTempo {
                tempo: Tempo::new(90_000_000).unwrap(),
            }))
            .unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        let mut terminal = None;
        let mut cut = None;
        let mut held_blocks = 2;
        while held_blocks > 0 || terminal.is_none() {
            playback.pump();
            if let Ok(event) = playback.service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                if event.state.revision > applied.state.revision {
                    terminal = Some(event);
                }
            }
            let status = playback.audio.as_ref().unwrap().status().unwrap();
            let before_position = status.position.load(Acquire);
            let before_frame = status.performance_frame.load(Acquire);
            let before_revision = status.applied_revision.load(Acquire);
            let pcm = render_completed_block(&playback);
            assert!(!status.stream_buffering.load(Acquire));
            assert_eq!(status.performance_frame.load(Acquire), before_frame + 256);
            if before_revision == revision && status.applied_revision.load(Acquire) > revision {
                cut = Some((before_position, before_frame));
            }
            if held_blocks > 0 {
                assert_eq!(
                    pcm,
                    *expected
                        .get((before_frame / 256) as usize)
                        .expect("tempo edit exceeded the uninterrupted PCM oracle"),
                    "in-flight PCM at frame {before_frame}, cut {cut:?}"
                );
                if cut.is_some() {
                    held_blocks -= 1;
                }
            }
            assert!(Instant::now() < until, "loop tempo revision did not commit");
        }

        let (old_position, cut_frame) = cut.unwrap();
        let terminal = terminal.unwrap();
        let edited = &terminal.state.project;
        let status = playback.audio.as_ref().unwrap().status().unwrap();
        let region = edited.loop_region().unwrap();
        assert_eq!((region.start(), region.end()), (1024, 3072));
        let new_position = edited
            .tempo_map()
            .beat_to_frame(old_tempo.frame_to_beat(old_position).unwrap())
            .unwrap();
        let actual_cursor = |frames: u64| {
            region.start()
                + (new_position - region.start() + frames) % (region.end() - region.start())
        };
        assert_eq!(
            status.position.load(Acquire),
            actual_cursor(status.performance_frame.load(Acquire) - cut_frame)
        );
        for _ in 0..9 {
            let pcm = render_completed_block(&playback);
            assert!(
                pcm.iter().any(|frame| *frame != [0.0; 2]),
                "a full quantum was silent"
            );
            assert_eq!(
                status.position.load(Acquire),
                actual_cursor(status.performance_frame.load(Acquire) - cut_frame)
            );
        }
        assert!(status.playing.load(Acquire) && status.rejected_revision().is_none());
    }
}

fn assert_pcm(ordinal: usize, block: &[hex_engine::Frame; 256]) {
    // The original effect fixture increases its gain on every successful SDK
    // call. The input is 0.25 and the bridge delays it by exactly two quanta.
    let expected = if ordinal < 2 {
        0.0
    } else {
        0.25 * (0.5 + (ordinal - 2) as f32 * 0.125)
    };
    for (offset, frame) in block.iter().enumerate() {
        assert_eq!(*frame, [expected; 2], "block {ordinal}, offset {offset}");
    }
}
