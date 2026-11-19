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

fn run(streamed: bool) {
    let files = Files::new();
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
    let imported = h.run(Request::Import(files.wav(48_000, frames)));
    let (imported_revision, _) = h.applied(initial);
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
    let original_map = accepted.state.project.tempo_map().clone();

    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
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
