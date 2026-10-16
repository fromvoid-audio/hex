use super::*;
use std::sync::atomic::Ordering;
fn fixture() -> (
    Session,
    hex_project::ClipId,
    Arc<hex_engine::PreparedRevision>,
) {
    let mut session = Session::new(16_000, 32, 8_192).unwrap();
    let audio = Audio::new(
        16_000,
        (0..8_000)
            .map(|i| {
                let x = (i as f32 * 0.173).sin() * 0.4;
                [x, -x]
            })
            .collect(),
    )
    .unwrap();
    let waveform = waveform::summarize(&audio, 1_024).unwrap();
    let prepared = PreparedImport {
        path: "root.wav".into(),
        source_sample_rate: 16_000,
        source_frames: 8_000,
        conversion_version: hex_project::IDENTITY_CONVERSION,
        audio,
        waveform,
    };
    let (Created::Imported { clip, .. }, revision) =
        session.import_prepared_revision(prepared, None).unwrap()
    else {
        panic!("import")
    };
    (session, clip, revision)
}
#[test]
fn accepted_waveforms_share_cropped_pcm_keep_gain_outside_and_failed_edit_keeps_history_and_peaks()
{
    let (mut session, clip, previous) = fixture();
    let (_, pitched) = session
        .command_prepared_revision(
            Command::SetAudioClipPitch { clip, cents: 137 },
            Some(&previous),
        )
        .unwrap();
    let root = Arc::clone(&session.audio_clip_waveforms.borrow()[&clip].waveform);
    let (Created::Clip(tail), split) = session
        .command_prepared_revision(Command::SplitClip { clip, frame: 3_137 }, Some(&pitched))
        .unwrap()
    else {
        panic!("split")
    };
    {
        let views = session.audio_clip_waveforms.borrow();
        assert!(Arc::ptr_eq(&root, &views[&tail].waveform));
        assert_eq!(views[&tail].offset, 3_137);
        assert_eq!(views[&tail].length, 4_863);
    }
    session
        .command_prepared_revision(
            Command::SetAudioClipGain {
                clip: tail,
                millidb: -6_000,
            },
            Some(&split),
        )
        .unwrap();
    assert!(Arc::ptr_eq(
        &root,
        &session.audio_clip_waveforms.borrow()[&tail].waveform
    ));
    let before = session.project().clone();
    let can_undo = session.can_undo();
    let can_redo = session.can_redo();
    session.shutdown.store(true, Ordering::Release);
    assert!(
        session
            .command_prepared_revision(
                Command::SetAudioClipPitch {
                    clip: tail,
                    cents: 211
                },
                Some(&split)
            )
            .is_err()
    );
    assert_eq!(session.project(), &before);
    assert_eq!(
        (session.can_undo(), session.can_redo()),
        (can_undo, can_redo)
    );
    assert!(Arc::ptr_eq(
        &root,
        &session.audio_clip_waveforms.borrow()[&tail].waveform
    ));
    let mut renderer = hex_engine::Renderer::new(split.arrangement());
    renderer.play();
    let mut actual = [[0.0; 2]; 31];
    renderer.render(&mut actual).unwrap();
    assert!(actual.iter().any(|frame| frame[0] != 0.0));
}
