use super::*;
use std::io::Write;

fn long_converted_source(files: &Files) -> PathBuf {
    let path = files.0.join("long-granular.wav");
    let mut file = fs::File::create(&path).unwrap();
    let data_bytes = 40_000_000u32; // 20M original frames, 40M at the project rate.
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36 + data_bytes).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&24_000u32.to_le_bytes()).unwrap();
    file.write_all(&48_000u32.to_le_bytes()).unwrap();
    file.write_all(&2u16.to_le_bytes()).unwrap();
    file.write_all(&16u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&data_bytes.to_le_bytes()).unwrap();
    for _ in 0..12_000 {
        file.write_all(&8192i16.to_le_bytes()).unwrap();
    }
    file.set_len(44 + u64::from(data_bytes)).unwrap();
    path
}

#[test]
fn verified_granular_edits_publish_worker_audio_in_both_modes() {
    let _native = native_fixture();
    for mode in [
        hex_project::WarpMode::Tones { grain_ms: 32 },
        hex_project::WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        let files = Files::new();
        let mut h = Harness::with_rate(48_000);
        h.next();
        h.open();
        h.run(Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(AudioPolicy::source_reservation()),
        });
        h.open();
        h.applied(h.context.unwrap());
        let imported = h.run(Request::Import(long_converted_source(&files)));
        h.applied(h.context.unwrap());
        let clip = imported.state.project.tracks()[0].clips()[0].id();
        let neutral = h.run(Request::Edit(hex_project::Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        }));
        h.applied(h.context.unwrap());
        let warp = hex_project::default_audio_warp(
            neutral.state.project.clip(clip).unwrap(),
            neutral.state.project.tempo_map(),
            mode,
        )
        .unwrap();
        let edited = h.run(Request::Edit(hex_project::Command::SetAudioClipWarp {
            clip,
            settings: Some(warp),
        }));
        assert!(edited.playback.is_none());
        assert_eq!(
            edited
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .candidate_reservation_quote()
                .unwrap()
                .prepared_pcm_frames,
            0
        );
        h.applied(h.context.unwrap());
        play_from_start_expecting(&mut h, 0.25);
        h.run(Request::Undo);
        h.applied(h.context.unwrap());
        play_from_start_expecting(&mut h, 0.25);
        h.run(Request::Redo);
        h.applied(h.context.unwrap());
        play_from_start_expecting(&mut h, 0.25);
        let original_key = edited
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .required_granular_roots(1)
            .next()
            .unwrap()
            .unwrap()
            .1
            .metadata()
            .key();
        let stretched = h.run(Request::Edit(hex_project::Command::SetProjectTempo {
            tempo: hex_project::Tempo::new(90_000_000).unwrap(),
        }));
        let stretched_source = stretched.state.canonical_audio_source.as_ref().unwrap();
        let stretched_key = stretched_source
            .required_granular_roots(1)
            .next()
            .unwrap()
            .unwrap()
            .1
            .metadata()
            .key();
        assert_ne!(
            original_key, stretched_key,
            "tempo change must select new synthesized pages"
        );
        assert_eq!(
            stretched_source
                .candidate_reservation_quote()
                .unwrap()
                .prepared_pcm_frames,
            0
        );
        h.applied(h.context.unwrap());
        play_from_start_expecting(&mut h, 0.25);
    }
}
