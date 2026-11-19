// Artifact-only normal-command fixture; not compiled or run during the EQ freeze.
#[test]
fn occurrence_fade_uses_its_processing_origin_and_retained_author_clock() {
    use hex_project::{
        AudioViewResizeOperation, BeatTime, ClipFade, Command, Created, Editor, Project, Tempo,
    };
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "original transient".into(),
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: 24_000,
        })
        .unwrap()
    else {
        panic!("audio import")
    };
    for command in [
        Command::SetAudioClipMusical {
            clip,
            enabled: true,
        },
        Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: 96_000,
            },
        },
        Command::SetClipFades {
            clip,
            fade_in: 18_000,
            fade_out: 6_000,
        },
    ] {
        editor.execute(command).unwrap();
    }
    // Author capture stays at the old120BPM law while later occurrences process
    // under90BPM. Query the sample position through the owning tempo map.
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ONE,
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let project = editor.project();
    let canonical = project.clip(clip).unwrap();
    let projection = AudioViewProjection::new(canonical, project.tempo_map()).unwrap();
    let authored = canonical.retained_fade();
    let local = BeatTime::ONE.raw() / 4;
    for index in 0..projection.occurrence_count() {
        let occurrence = projection.occurrence(index).unwrap();
        let beat = u64::try_from(occurrence.root_origin + i128::from(local)).unwrap();
        let frame = project
            .tempo_map()
            .beat_to_frame(BeatTime::from_raw(beat))
            .unwrap();
        let root_frame = u64::try_from(i128::from(frame) - occurrence.root_frame_origin).unwrap();
        let fade = projection.occurrence_fade(index).unwrap();
        // One quarter of the captured quarter-note is6000frames at120BPM.
        assert_eq!(fade.gain_at(root_frame).unwrap(), authored.gain_at(6_000));
        assert!(fade.gain_at(occurrence.root_frame_length).is_err());
    }
    // Remaining consumer work: musical held/pending/canonical shade mesh parity,
    // frozen-root source fixture, and explicit directed destination-clock study.
    let _ = ClipFade::DEFAULT_S_CURVE;
}
