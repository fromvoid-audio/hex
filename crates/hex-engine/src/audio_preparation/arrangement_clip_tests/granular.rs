//! Exact canonical granular binding through semantic and native preparation.
use super::*;
use hex_project::{Command, Created, Editor, Project, WarpMode, default_audio_warp};
fn fixture(
    mode: WarpMode,
    converted: bool,
    frames: u64,
) -> (
    Editor,
    hex_project::ClipId,
    crate::StreamedSourceMetadata,
    crate::StreamedSourceMetadata,
) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let source_rate = if converted { 44_100 } else { 48_000 };
    let playback = hex_project::converted_frame_count(frames, source_rate, 48_000).unwrap();
    let conversion = if converted {
        hex_project::SINC_CONVERSION_V2
    } else {
        hex_project::IDENTITY_CONVERSION
    };
    let Created::Imported { clip, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "granular".into(),
            path: "granular.wav".into(),
            source_sample_rate: source_rate,
            source_frames: frames,
            playback_frames: playback,
            conversion_version: conversion,
        })
        .unwrap()
    else {
        panic!("audio fixture");
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    // Full long processing roots remain a separate Project/schema contract.
    // This fixture's long-file case selects a legal bounded source region.
    if frames > hex_project::MAX_AUDIO_DOMAIN_FRAMES {
        editor
            .execute(Command::TrimClip {
                clip,
                start: 0,
                duration: 8192,
                source_offset: 0,
                source_length: 8192,
            })
            .unwrap();
    }
    let warp = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        mode,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(warp),
        })
        .unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: hex_project::TrimEdge::Left,
            frame: 1_000,
        })
        .unwrap();
    let raw = crate::StreamedSourceMetadata::new(
        crate::StreamSourceKey::new(11, asset),
        source_rate,
        48_000,
        frames,
        playback,
        conversion,
        [7; 32],
    )
    .unwrap();
    let derived = crate::audio_transform::streamed_granular_root(
        &raw,
        editor.project().clip(clip).unwrap().audio_edit().unwrap(),
        editor.project().tempo_map(),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap()
    .metadata()
    .clone();
    (editor, clip, raw, derived)
}
fn semantic(
    project: &Project,
    raw: &crate::StreamedSourceMetadata,
    derived: &[crate::StreamedSourceMetadata],
) -> Result<crate::PreparedRevision, crate::Invalid> {
    let sources = std::collections::BTreeMap::from([(
        raw.key().asset(),
        crate::compile::PreparedAudioSource::Streamed(Arc::new(raw.clone())),
    )]);
    let derived = derived
        .iter()
        .map(|m| (m.key(), Arc::new(m.clone())))
        .collect();
    crate::PreparedRevision::prepare_with_prepared_pitch_sources(
        project,
        &sources,
        &derived,
        11,
        Arc::new(crate::SineTable::new()),
        None,
    )
}
#[test]
fn granular_roots_bind_semantic_and_native_views_without_prepared_pcm() {
    for mode in [
        WarpMode::Tones { grain_ms: 32 },
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        for converted in [false, true] {
            let (editor, clip, raw, derived) = fixture(mode, converted, 8192);
            let project = editor.project();
            let (_, _, start, end) = project
                .clip(clip)
                .unwrap()
                .audio_edit()
                .unwrap()
                .projected_domain(project.tempo_map())
                .unwrap();
            let revision = semantic(project, &raw, std::slice::from_ref(&derived)).unwrap();
            let prepared = &revision.arrangement().tracks()[0].clips[0];
            assert_eq!(
                prepared.metadata.as_ref().unwrap().streamed.as_deref(),
                Some(&derived)
            );
            assert_eq!(prepared.source_offset as u64, start);
            assert_eq!(prepared.source_length as u64, end - start);
            let quote = crate::AudioCandidateReservationQuote::for_project_with_pitched_streams(
                project,
                2,
                &[raw.clone(), derived.clone()],
            )
            .unwrap();
            assert_eq!(quote.prepared_pcm_frames, 0);
            let reservation = quote.storage_reservation(4096).unwrap();
            let mut candidate =
                crate::AudioCandidatePreparation::try_new_on_control(2, reservation).unwrap();
            assert_eq!(
                candidate.add_granular_streamed_source_on_control(derived.clone()),
                Err(AudioStorageError::SourceMismatch)
            );
            candidate
                .add_streamed_source_on_control(raw.clone())
                .unwrap();
            candidate
                .add_granular_streamed_source_on_control(derived.clone())
                .unwrap();
            assert!(
                candidate
                    .add_granular_streamed_source_on_control(derived.clone())
                    .is_err()
            );
            candidate
                .prepare_on_control(
                    project,
                    11,
                    crate::AudioSourceContext {
                        project_generation: 2,
                        editor_revision: 8,
                    },
                )
                .unwrap();
            assert_eq!(candidate.resident_pcm_capacity_frames().unwrap(), 0);
            candidate.cleanup_on_control().unwrap();
            let bank = OwnedAudioInputs::try_new_with_derived(
                vec![
                    (
                        raw.key().asset(),
                        OwnedAudioSource::try_streamed(raw.clone()).ok().unwrap(),
                    ),
                    (
                        raw.key().asset(),
                        OwnedAudioSource::try_granular_streamed(derived.clone())
                            .ok()
                            .unwrap(),
                    ),
                ],
                reservation,
            )
            .ok()
            .unwrap();
            let token = bank.granular_source_token(derived.key()).unwrap();
            assert_eq!(
                bank.source_view(&token).unwrap().streamed_metadata(),
                Some(&derived)
            );
            assert!(bank.granular_source_token(raw.key()).is_err());
            let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
            let token = begin(&mut custody, project, bank).ok().unwrap();
            let prepared = custody.arrangement_clips.clips().next().unwrap();
            assert_eq!(
                prepared.metadata.as_ref().unwrap().streamed.as_deref(),
                Some(&derived)
            );
            assert_eq!(prepared.source_offset as u64, start);
            assert_eq!(prepared.source_length as u64, end - start);
            assert_eq!(custody.resident_pcm_capacity_frames().unwrap(), 0);
            assert!(custody.cancel(&token));
            custody.cleanup_on_control();
            assert!(custody.recover_on_control(&token).is_some());
        }
    }
}
#[test]
fn granular_identity_and_missing_sources_never_fall_back_to_raw_or_resident_audio() {
    let (mut editor, clip, raw, derived) = fixture(WarpMode::Tones { grain_ms: 32 }, false, 8192);
    assert!(OwnedAudioSource::try_streamed(derived.clone()).is_err());
    assert!(OwnedAudioSource::try_repitch_streamed(derived.clone()).is_err());
    assert!(OwnedAudioSource::try_granular_streamed(raw.clone()).is_err());
    assert!(semantic(editor.project(), &raw, &[]).is_err());
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 300 })
        .unwrap();
    assert!(semantic(editor.project(), &raw, std::slice::from_ref(&derived)).is_err());
    let changed = crate::audio_transform::streamed_granular_root(
        &raw,
        editor.project().clip(clip).unwrap().audio_edit().unwrap(),
        editor.project().tempo_map(),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap()
    .metadata()
    .clone();
    let quote = crate::AudioCandidateReservationQuote::for_project_with_pitched_streams(
        editor.project(),
        2,
        &[raw.clone(), changed],
    )
    .unwrap();
    assert_eq!(quote.prepared_pcm_frames, 0);
    let mut candidate = crate::AudioCandidatePreparation::try_new_on_control(
        2,
        quote.storage_reservation(4096).unwrap(),
    )
    .unwrap();
    candidate
        .add_streamed_source_on_control(raw.clone())
        .unwrap();
    candidate
        .add_granular_streamed_source_on_control(derived.clone())
        .unwrap();
    assert!(
        candidate
            .prepare_on_control(
                editor.project(),
                11,
                crate::AudioSourceContext {
                    project_generation: 2,
                    editor_revision: 8
                }
            )
            .is_err()
    );
    assert!(candidate.resident_pcm_capacity_frames().is_err()); // no prepared owner escaped refusal
    candidate.cleanup_on_control().unwrap();
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 8,
            fade_out: 8,
        })
        .unwrap();
    assert!(semantic(editor.project(), &raw, std::slice::from_ref(&derived)).is_err());
}
#[test]
fn granular_banks_refuse_foreign_raw_facts_and_wrong_order_without_losing_source_owners() {
    let (editor, _, raw, derived) = fixture(
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
        true,
        8192,
    );
    let quote = crate::AudioCandidateReservationQuote::for_project_with_pitched_streams(
        editor.project(),
        2,
        &[raw.clone(), derived.clone()],
    )
    .unwrap();
    let reservation = quote.storage_reservation(4096).unwrap();
    for case in 0..3 {
        let foreign = if case == 0 {
            crate::StreamedSourceMetadata::new(
                raw.key(),
                raw.source_rate(),
                raw.project_rate(),
                raw.source_frames(),
                raw.playback_frames(),
                raw.conversion_version(),
                [8; 32],
            )
            .unwrap()
        } else if case == 1 {
            crate::StreamedSourceMetadata::new(
                crate::StreamSourceKey::new(12, raw.key().asset()),
                raw.source_rate(),
                raw.project_rate(),
                raw.source_frames(),
                raw.playback_frames(),
                raw.conversion_version(),
                raw.fingerprint(),
            )
            .unwrap()
        } else {
            raw.clone()
        };
        assert_eq!(derived.is_granular_of(&foreign), case == 2);
        let mut sources = vec![
            (
                raw.key().asset(),
                OwnedAudioSource::try_streamed(foreign).ok().unwrap(),
            ),
            (
                raw.key().asset(),
                OwnedAudioSource::try_granular_streamed(derived.clone())
                    .ok()
                    .unwrap(),
            ),
        ];
        if case == 2 {
            sources.swap(0, 1);
        }
        let pointer = sources.as_ptr();
        let mut destination = None;
        assert_eq!(
            OwnedAudioInputs::try_new_with_derived_in_place(
                &mut sources,
                reservation,
                &mut destination
            ),
            Err(AudioStorageError::SourceMismatch)
        );
        assert_eq!(sources.as_ptr(), pointer);
        assert_eq!(sources.len(), 2);
        assert!(destination.is_none());
    }
}
#[test]
fn long_source_with_bounded_granular_region_needs_no_resident_domain_expansion() {
    let (editor, _, raw, derived) = fixture(WarpMode::Tones { grain_ms: 32 }, false, 20_000_000);
    let quote = crate::AudioCandidateReservationQuote::for_project_with_pitched_streams(
        editor.project(),
        2,
        &[raw.clone(), derived.clone()],
    )
    .unwrap();
    assert_eq!(quote.prepared_pcm_frames, 0);
    let mut candidate = crate::AudioCandidatePreparation::try_new_on_control(
        2,
        quote.storage_reservation(4096).unwrap(),
    )
    .unwrap();
    candidate.add_streamed_source_on_control(raw).unwrap();
    candidate
        .add_granular_streamed_source_on_control(derived)
        .unwrap();
    candidate
        .prepare_on_control(
            editor.project(),
            11,
            crate::AudioSourceContext {
                project_generation: 2,
                editor_revision: 8,
            },
        )
        .unwrap();
    assert_eq!(candidate.resident_pcm_capacity_frames().unwrap(), 0);
    candidate.cleanup_on_control().unwrap();
}
