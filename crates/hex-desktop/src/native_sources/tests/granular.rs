use super::*;
use hex_project::{Command, WarpMode};
#[test]
fn granular_projection_deduplicates_and_collects_exact_converted_roots() {
    for warp_mode in [
        WarpMode::Tones { grain_ms: 32 },
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        let files = Files::new();
        let service = Service::new(48_000, 16, 0).unwrap();
        next(&service);
        run(&service, mode());
        let imported = run(&service, Request::Import(files.wav("granular.wav", 1000)));
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
            warp_mode,
        )
        .unwrap();
        run(
            &service,
            Request::Edit(Command::SetAudioClipWarp {
                clip,
                settings: Some(warp),
            }),
        );
        let duplicated = run(&service, Request::Edit(Command::DuplicateClip(clip)));
        let snapshot = duplicated.state.canonical_audio_source.as_ref().unwrap();
        let domain = DomainSources::capture(snapshot, 11).unwrap();
        assert_eq!(
            domain.members.len(),
            2,
            "raw plus one deduplicated granular root"
        );
        assert!(domain.matches_snapshot(snapshot));
        for requirement in snapshot.required_granular_roots(11) {
            let (file, root) = requirement.unwrap();
            assert_eq!(root.metadata().source_rate(), 24_000);
            assert_eq!(root.metadata().project_rate(), 48_000);
            assert!(domain.contains_member(file, root.metadata()));
        }
        assert!(domain.definitions().is_err());
        assert!(
            domain
                .definitions_for_snapshot(snapshot)
                .unwrap_err()
                .contains("granular")
        );
        let quote = snapshot.candidate_reservation_quote().unwrap();
        assert_eq!(quote.prepared_pcm_frames, 0);
        let mut candidate = hex_engine::AudioCandidatePreparation::try_new_on_control(
            2,
            quote.storage_reservation(0).unwrap(),
        )
        .unwrap();
        for member in domain
            .members
            .iter()
            .filter(|member| member.metadata.key().is_raw())
        {
            candidate
                .add_streamed_source_on_control(member.metadata.clone())
                .unwrap();
        }
        domain
            .add_required_derived(snapshot, &mut candidate)
            .unwrap();
        snapshot
            .prepare_candidate_on_control(11, &mut candidate)
            .unwrap();
        assert_eq!(candidate.resident_pcm_capacity_frames().unwrap(), 0);
        candidate.cleanup_on_control().unwrap();
        let duplicate = duplicated.state.project.tracks()[0]
            .clips()
            .iter()
            .find(|other| other.id() != clip)
            .unwrap()
            .id();
        let changed = run(
            &service,
            Request::Edit(Command::SetAudioClipPitch {
                clip: duplicate,
                cents: 300,
            }),
        );
        let changed_snapshot = changed.state.canonical_audio_source.as_ref().unwrap();
        assert!(!domain.matches_snapshot(changed_snapshot));
        assert!(domain.definitions_for_snapshot(changed_snapshot).is_err());
        let changed_domain = DomainSources::capture(changed_snapshot, 11).unwrap();
        assert_eq!(changed_domain.members.len(), 3);
        assert!(changed_domain.matches_snapshot(changed_snapshot));
        assert_ne!(
            DomainSources::capture(changed_snapshot, 12)
                .unwrap()
                .members[1]
                .metadata,
            changed_domain.members[1].metadata
        );
    }
}
#[test]
fn incomplete_or_foreign_granular_facts_refuse_candidate_collection() {
    let files = Files::new();
    let service = Service::new(48_000, 16, 0).unwrap();
    next(&service);
    run(&service, mode());
    let imported = run(
        &service,
        Request::Import(files.wav("granular-refusal.wav", 1000)),
    );
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
        WarpMode::Tones { grain_ms: 32 },
    )
    .unwrap();
    let edited = run(
        &service,
        Request::Edit(Command::SetAudioClipWarp {
            clip,
            settings: Some(warp),
        }),
    );
    let snapshot = edited.state.canonical_audio_source.as_ref().unwrap();
    let quote = snapshot.candidate_reservation_quote().unwrap();
    for case in 0..3 {
        let mut domain = DomainSources::capture(snapshot, if case == 1 { 12 } else { 11 }).unwrap();
        domain.catalog_generation = 11;
        if case == 0 {
            domain
                .members
                .retain(|member| member.metadata.key().is_raw());
        }
        if case == 2 {
            let (file, root) = snapshot
                .required_granular_roots(11)
                .next()
                .unwrap()
                .unwrap();
            let expected = root.metadata().clone();
            let (_, region, map, mode, cents, law) = root.into_parts();
            let raw = file.source_metadata(expected.key().asset(), 11).unwrap();
            let mut fingerprint = raw.fingerprint();
            fingerprint[0] ^= 1;
            let foreign = hex_engine::StreamedSourceMetadata::new(
                raw.key(),
                raw.source_rate(),
                raw.project_rate(),
                raw.source_frames(),
                raw.playback_frames(),
                raw.conversion_version(),
                fingerprint,
            )
            .unwrap()
            .with_granular(&region, &map, mode, cents, law)
            .unwrap();
            domain
                .members
                .iter_mut()
                .find(|member| member.metadata.key() == expected.key())
                .unwrap()
                .metadata = foreign;
        }
        assert!(!domain.matches_snapshot(snapshot));
        let mut candidate = hex_engine::AudioCandidatePreparation::try_new_on_control(
            2,
            quote.storage_reservation(0).unwrap(),
        )
        .unwrap();
        for (asset, binding) in snapshot.required_sources() {
            let AudioSourceBinding::VerifiedFile(file) = binding else {
                panic!("file binding")
            };
            candidate
                .add_streamed_source_on_control(file.source_metadata(asset, 11).unwrap())
                .unwrap();
        }
        assert_eq!(
            domain.add_required_derived(snapshot, &mut candidate),
            Err(hex_engine::AudioStorageError::SourceMismatch)
        );
        candidate.cleanup_on_control().unwrap();
    }
}
