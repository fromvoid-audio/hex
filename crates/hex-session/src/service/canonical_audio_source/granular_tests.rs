//! Captured granular requirements retain canonical edit and file identity.
use super::*;
use crate::audio_sources::granular_tests::Fixture;
use hex_project::{Command, Created, WarpMode};
fn snapshot(fixture: &Fixture) -> CanonicalAudioSourceSnapshot {
    CanonicalAudioSourceSnapshot {
        project: Arc::new(fixture.editor.project().clone()),
        context: hex_engine::AudioSourceContext {
            project_generation: 2,
            editor_revision: 3,
        },
        mode: crate::DocumentAudioMode::VerifiedStreaming,
        sources: BTreeMap::from([(
            fixture.asset,
            crate::AudioSourceBinding::VerifiedFile(fixture.file.clone()),
        )]),
    }
}
fn keys(
    snapshot: &CanonicalAudioSourceSnapshot,
    generation: u64,
) -> Vec<hex_engine::StreamSourceKey> {
    snapshot
        .required_granular_roots(generation)
        .map(|root| root.unwrap().1.metadata().key())
        .collect()
}
#[test]
fn granular_snapshot_quotes_distinct_roots_and_restores_identity_through_history() {
    for mode in [
        WarpMode::Tones { grain_ms: 32 },
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        let mut fixture = Fixture::new(mode, true);
        let before = snapshot(&fixture);
        let original = keys(&before, 19);
        assert_eq!(original.len(), 1);
        assert_ne!(original, keys(&before, 20));
        assert_eq!(before.required_repitch_sources().count(), 0);
        assert_eq!(before.required_plain_pitch_sources().count(), 0);
        let Created::Clip(duplicate) = fixture
            .editor
            .execute(Command::DuplicateClip(fixture.clip))
            .unwrap()
        else {
            panic!("duplicate")
        };
        let duplicate_snapshot = snapshot(&fixture);
        assert_eq!(keys(&duplicate_snapshot, 19), vec![original[0]; 2]);
        assert_eq!(
            duplicate_snapshot
                .candidate_reservation_quote()
                .unwrap()
                .source_capacity,
            2
        );
        fixture
            .editor
            .execute(Command::SetAudioClipPitch {
                clip: duplicate,
                cents: 300,
            })
            .unwrap();
        let changed = snapshot(&fixture);
        let changed_keys = keys(&changed, 19);
        assert_eq!(changed_keys[0], original[0]);
        assert_ne!(changed_keys[0], changed_keys[1]);
        let quote = changed.candidate_reservation_quote().unwrap();
        assert_eq!(quote.source_capacity, 3);
        assert_eq!(quote.prepared_pcm_frames, 0);
        let mut candidate = hex_engine::AudioCandidatePreparation::try_new_on_control(
            3,
            quote.storage_reservation(4096).unwrap(),
        )
        .unwrap();
        candidate
            .add_streamed_source_on_control(
                fixture.file.source_metadata(fixture.asset, 19).unwrap(),
            )
            .unwrap();
        for root in changed.required_granular_roots(19) {
            let (file, root) = root.unwrap();
            assert_eq!(file, &fixture.file);
            candidate
                .add_granular_streamed_source_on_control(root.metadata().clone())
                .unwrap();
        }
        changed
            .prepare_candidate_on_control(19, &mut candidate)
            .unwrap();
        assert_eq!(candidate.resident_pcm_capacity_frames().unwrap(), 0);
        candidate.cleanup_on_control().unwrap();
        assert!(fixture.editor.undo());
        assert_eq!(keys(&snapshot(&fixture), 19), vec![original[0]; 2]);
        assert!(fixture.editor.redo());
        assert_eq!(keys(&snapshot(&fixture), 19), changed_keys);
        assert_eq!(
            keys(&before, 19),
            original,
            "earlier immutable snapshot remains exact"
        );
    }
}
#[test]
fn unsupported_snapshot_root_reports_error_instead_of_omitting_requirement() {
    let mut fixture = Fixture::new(
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
        false,
    );
    fixture
        .editor
        .execute(Command::SetClipFades {
            clip: fixture.clip,
            fade_in: 8,
            fade_out: 8,
        })
        .unwrap();
    let source = snapshot(&fixture);
    let roots: Vec<_> = source.required_granular_roots(19).collect();
    assert_eq!(roots.len(), 1);
    assert!(roots[0].is_err());
    assert!(source.candidate_reservation_quote().is_err());
}

#[test]
fn verified_session_commits_granular_edits_and_refusal_preserves_history_and_source() {
    use super::super::verified_source_tests::{next, reservation, run};
    for mode in [
        WarpMode::Tones { grain_ms: 32 },
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        let fixture = Fixture::new(mode, true);
        let original_bytes = std::fs::read(fixture.file.path()).unwrap();
        let service = Service::new(48_000, 16, 0).unwrap();
        next(&service);
        run(
            &service,
            Request::NewWithAudioMode {
                mode: crate::DocumentAudioMode::VerifiedStreaming,
                reservation: Some(reservation(2)),
            },
        );
        let imported = run(&service, Request::Import(fixture.file.path().to_path_buf()));
        let clip = imported.state.project.tracks()[0].clips()[0].id();
        let neutral = run(
            &service,
            Request::Edit(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            }),
        );
        let warp = hex_project::default_audio_warp(
            neutral.state.project.clip(clip).unwrap(),
            neutral.state.project.tempo_map(),
            mode,
        )
        .unwrap();
        let warped = run(
            &service,
            Request::Edit(Command::SetAudioClipWarp {
                clip,
                settings: Some(warp),
            }),
        );
        let expected = keys(warped.state.canonical_audio_source.as_ref().unwrap(), 19);
        assert_eq!(expected.len(), 1);
        let undone = run(&service, Request::Undo);
        assert!(keys(undone.state.canonical_audio_source.as_ref().unwrap(), 19).is_empty());
        let redone = run(&service, Request::Redo);
        let snapshot = redone.state.canonical_audio_source.as_ref().unwrap();
        assert_eq!(keys(snapshot, 19), expected);
        assert_eq!(
            snapshot
                .candidate_reservation_quote()
                .unwrap()
                .prepared_pcm_frames,
            0
        );
        assert!(
            snapshot
                .sources()
                .values()
                .all(|source| matches!(source, crate::AudioSourceBinding::VerifiedFile(_)))
        );
        service
            .submit(Request::Edit(Command::SetClipFades {
                clip,
                fade_in: 8,
                fade_out: 8,
            }))
            .unwrap();
        let refused = next(&service);
        assert!(refused.error.is_some());
        assert_eq!(
            refused.state.project.snapshot(),
            redone.state.project.snapshot()
        );
        let undone = run(&service, Request::Undo);
        assert_eq!(
            undone.state.project.snapshot(),
            neutral.state.project.snapshot()
        );
        assert_eq!(std::fs::read(fixture.file.path()).unwrap(), original_bytes);
    }
}

#[test]
fn full_long_verified_granular_snapshot_prepares_zero_pcm_candidate() {
    for mode in [
        WarpMode::Tones { grain_ms: 32 },
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        let fixture = Fixture::with_frames(mode, false, 20_000_000);
        let source = snapshot(&fixture);
        let quote = source.candidate_reservation_quote().unwrap();
        assert_eq!(quote.prepared_pcm_frames, 0);
        let mut candidate = hex_engine::AudioCandidatePreparation::try_new_on_control(
            2,
            quote.storage_reservation(4096).unwrap(),
        )
        .unwrap();
        candidate
            .add_streamed_source_on_control(
                fixture.file.source_metadata(fixture.asset, 19).unwrap(),
            )
            .unwrap();
        let roots: Vec<_> = source
            .required_granular_roots(19)
            .map(Result::unwrap)
            .collect();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].1.metadata().playback_frames(), 20_000_000);
        candidate
            .add_granular_streamed_source_on_control(roots[0].1.metadata().clone())
            .unwrap();
        source
            .prepare_candidate_on_control(19, &mut candidate)
            .unwrap();
        assert_eq!(candidate.resident_pcm_capacity_frames().unwrap(), 0);
        candidate.cleanup_on_control().unwrap();
    }
}
