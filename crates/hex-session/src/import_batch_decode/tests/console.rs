use super::*;
use crate::{
    DocumentAudioMode, SourcePlanReservation,
    import_batch::{AudioBatchDestination, ConsoleDestination},
};
use hex_project::{Command, Created, SceneId, SessionCellContent, TrackId};

fn fixture(mode: DocumentAudioMode) -> (Session, TrackId, SceneId) {
    let reservation = SourcePlanReservation {
        reader: hex_media::VerifiedWavReaderLimits {
            reader: hex_media::WavReaderLimits {
                max_file_bytes: 64 * 1024 * 1024,
                max_source_frames: 8_000_000,
                max_header_count: 32,
                max_window_frames: 4096,
            },
            max_manifest_bytes: 1024 * 1024,
        },
        max_retained_sources: 16,
        max_path_bytes: 1024 * 1024,
        max_manifest_bytes: 4 * 1024 * 1024,
    };
    let mut s =
        Session::new_with_audio_mode(48_000, 32, 8_000_000, mode, Some(reservation)).unwrap();
    let Created::Track(track) = s
        .editor
        .execute(Command::InsertTrackWithDevice {
            name: "target".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("track")
    };
    // Verified audio-only source preparation does not grant native instruments.
    s.editor
        .execute(Command::RemoveInstrument { track })
        .unwrap();
    let Created::Scene(scene) = s
        .editor
        .execute(Command::AddScene {
            name: "first".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    (s, track, scene)
}
fn run(
    s: &mut Session,
    path: PathBuf,
    target: ConsoleDestination,
) -> (Created, Arc<hex_engine::PreparedRevision>) {
    let op = AudioBatchImport::capture(
        vec![path],
        AudioBatchDestination::Console(target),
        s,
        context(),
    )
    .unwrap();
    let (mut results, prepared) = op.run(s, None, context).unwrap();
    assert_eq!(results.len(), 1);
    (results.remove(0), prepared)
}

#[test]
fn console_actual_resident_and_verified_empty_replace_history_and_typed_assets() {
    for mode in [
        DocumentAudioMode::Resident,
        DocumentAudioMode::VerifiedStreaming,
    ] {
        let files = Files::new();
        let first = files.wav("First.WAV", 48_000, 512, 8192);
        let second = files.wav("Second.WAV", 48_000, 256, -8192);
        let (mut s, track, scene) = fixture(mode);
        let target = ConsoleDestination::Cell { track, scene };
        let (first_result, prepared) = run(&mut s, first.clone(), target);
        let Created::Imported {
            asset: old_asset,
            clip: old_clip,
            ..
        } = first_result
        else {
            panic!("typed import")
        };
        let audio = prepared
            .session()
            .audio_clip(old_clip)
            .expect("actual prepared Session audio");
        assert_eq!(
            (audio.track, audio.scene, audio.asset),
            (track, scene, old_asset)
        );
        if mode == DocumentAudioMode::Resident {
            assert!(audio.resident_audio().unwrap().frames()[1][0] > 0.0);
            let mut playback =
                hex_engine::playback::Playback::try_from_prepared_with_session(prepared).unwrap();
            playback
                .try_apply_action(hex_engine::PlaybackAction::LaunchAndPlay(
                    hex_engine::SessionPlaybackCommand::LaunchScene {
                        scene,
                        quantization: hex_engine::LaunchQuantization::Immediate,
                    },
                ))
                .unwrap();
            let mut block = [[0.0; 2]; 128];
            playback.render(&mut block).unwrap();
            assert!(
                block.iter().any(|f| f[0] > 0.0),
                "actual launch renders imported PCM"
            );
        } else {
            assert!(audio.resident_audio().is_none());
            assert!(s.verified_sources.contains_key(&old_asset));
            assert_eq!(s.resident_frames(), 0);
        }
        let before = s.project().clone();
        let old_bank = s.media.len() + s.verified_sources.len();
        let (result, _) = run(&mut s, second.clone(), target);
        let Created::Imported {
            asset: new_asset,
            clip: new_clip,
            ..
        } = result
        else {
            panic!("typed import")
        };
        assert_ne!(old_asset, new_asset);
        assert_ne!(old_clip, new_clip);
        assert_eq!(s.project().session_cells().len(), 1);
        assert_eq!(s.project().scenes(), before.scenes());
        assert_eq!(
            s.media.len() + s.verified_sources.len(),
            old_bank + 1,
            "old-new overlap stays retained for Undo"
        );
        let after = s.project().session_cells().to_vec();
        assert!(s.undo_prepared_revision(None).unwrap().is_some());
        assert_eq!(s.project().session_cells(), before.session_cells());
        assert!(s.redo_prepared_revision(None).unwrap().is_some());
        assert_eq!(s.project().session_cells(), after);
        assert_eq!(fs::read(first).unwrap()[..4], *b"RIFF");
        assert_eq!(fs::read(second).unwrap()[..4], *b"RIFF");
    }
}

#[test]
fn console_insert_before_and_bottom_create_one_atomic_scene_in_both_modes() {
    for mode in [
        DocumentAudioMode::Resident,
        DocumentAudioMode::VerifiedStreaming,
    ] {
        for before_anchor in [true, false] {
            let files = Files::new();
            let path = files.wav("Insert.wav", 48_000, 8, 8192);
            let (mut s, track, anchor) = fixture(mode);
            let before = s.project().clone();
            run(
                &mut s,
                path,
                ConsoleDestination::InsertScene {
                    track,
                    before: before_anchor.then_some(anchor),
                },
            );
            assert_eq!(s.project().scenes().len(), 2);
            let inserted = s.project().scenes()[usize::from(!before_anchor)].id();
            assert_ne!(inserted, anchor);
            assert!(matches!(
                s.project().session_cell(track, inserted),
                Some(SessionCellContent::Audio(_))
            ));
            assert!(s.project().session_cell(track, anchor).is_none());
            assert!(s.undo_prepared_revision(None).unwrap().is_some());
            assert_eq!(s.project().scenes(), before.scenes());
            assert_eq!(s.project().session_cells(), before.session_cells());
            assert!(s.redo_prepared_revision(None).unwrap().is_some());
            assert_eq!(s.project().scenes().len(), 2);
        }
    }
}

#[test]
fn console_failure_cancel_stale_generation_and_deleted_targets_preserve_project_media_history() {
    for mode in [
        DocumentAudioMode::Resident,
        DocumentAudioMode::VerifiedStreaming,
    ] {
        for failure in 0..5 {
            let files = Files::new();
            let good = files.wav("Old.wav", 48_000, 8, 8192);
            let bad = files.bad("Unsupported.wav");
            let (mut s, track, scene) = fixture(mode);
            run(
                &mut s,
                good.clone(),
                ConsoleDestination::Cell { track, scene },
            );
            let path = if failure == 0 { bad } else { good };
            let op = AudioBatchImport::capture(
                vec![path],
                AudioBatchDestination::Console(ConsoleDestination::Cell { track, scene }),
                &s,
                context(),
            )
            .unwrap();
            if failure == 1 {
                op.control().request_cancel();
            }
            if failure == 3 {
                s.editor.execute(Command::RemoveScene(scene)).unwrap();
            }
            if failure == 4 {
                s.editor.execute(Command::RemoveTrack(track)).unwrap();
            }
            let before = s.project().clone();
            let bank = s.media.len();
            let verified = s.verified_sources.len();
            let frames = s.resident_frames();
            let undo = s.can_undo();
            let redo = s.can_redo();
            let result = op.run(&mut s, None, || {
                let mut c = context();
                if failure == 2 {
                    c.project_generation += 1;
                }
                c
            });
            assert!(result.is_err(), "mode{mode:?},failure{failure}");
            unchanged(&s, &before, bank, frames);
            assert_eq!(s.verified_sources.len(), verified);
            assert_eq!((s.can_undo(), s.can_redo()), (undo, redo));
        }
    }
}

#[test]
fn console_insert_scene_stale_anchor_refuses_and_add_scene_still_appends() {
    let (mut s, _, first) = fixture(DocumentAudioMode::Resident);
    let Created::Scene(last) = s
        .editor
        .execute(Command::AddScene {
            name: "last".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::Scene(middle) = s
        .editor
        .execute(Command::InsertScene {
            name: "middle".into(),
            before: Some(last),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    assert_eq!(
        s.project()
            .scenes()
            .iter()
            .map(|s| s.id())
            .collect::<Vec<_>>(),
        [first, middle, last]
    );
    s.editor.execute(Command::RemoveScene(last)).unwrap();
    let before = s.project().clone();
    assert!(
        s.editor
            .execute(Command::InsertScene {
                name: "refused".into(),
                before: Some(last)
            })
            .is_err()
    );
    assert_eq!(s.project(), &before);
}
