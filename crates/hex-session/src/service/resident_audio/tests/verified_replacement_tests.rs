//! Mixed verified file and original resources on the ordinary control preparation path.
use super::*;
use crate::service::verified_source_tests::{
    Files as VerifiedFiles, reservation as file_reservation,
};

#[test]
fn verified_repitch_semantic_revision_keeps_raw_session_source() {
    use hex_project::{Command, Created, Editor, Project, WarpMode, default_audio_warp};
    let files = VerifiedFiles::new();
    let path = files.wav("repitch.wav", 16_000, 8_000, 1024);
    let file =
        crate::VerifiedAudioFile::inspect(path.clone(), 16_000, file_reservation(2).reader, || {
            false
        })
        .unwrap();
    let mut editor = Editor::new(Project::new(16_000).unwrap(), 8).unwrap();
    let Created::Imported { track, clip, asset } = editor
        .execute(Command::ImportAudioTrack {
            name: "file".into(),
            path,
            sample_rate: 16_000,
            frames: 8_000,
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let warp = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(warp),
        })
        .unwrap();
    assert!(matches!(
        editor.execute(Command::DuplicateClip(clip)).unwrap(),
        Created::Clip(_)
    ));
    let Created::Scene(scene) = editor
        .execute(Command::AddScene { name: "raw".into() })
        .unwrap()
    else {
        panic!("scene")
    };
    editor
        .execute(Command::CreateSessionAudioClip {
            track,
            scene,
            name: "raw".into(),
            asset,
            source_start: 0,
            source_end: 4_096,
        })
        .unwrap();
    let media = crate::audio_sources::MediaBank {
        epoch: 7,
        device_only: Default::default(),
        resident: BTreeMap::new(),
        verified: BTreeMap::from([(asset, file.clone())]),
    };
    let raw = file.source_metadata(asset, 7).unwrap();
    let edit = editor.project().clip(clip).unwrap().audio_edit().unwrap();
    let (map, law) = hex_engine::audio_transform::streamed_repitch_map(
        edit,
        editor.project().tempo_map(),
        raw.playback_frames(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let expected = raw.with_repitch(&map, law).unwrap();
    let revision = media
        .prepare(
            editor.project(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap();
    let keys: Vec<_> = revision
        .arrangement_stream_sources()
        .map(|source| source.key())
        .collect();
    assert_eq!(keys, vec![expected.key(), expected.key()]);
    let hex_engine::compile::PreparedAudioSource::Streamed(session_source) =
        revision.session().audio_clips()[0].source()
    else {
        panic!("session source")
    };
    assert_eq!(session_source.as_ref(), &raw);
    let catalog = ResidentFileMetadata::new_with_derived(
        7,
        BTreeMap::from([(asset, Arc::new(raw.clone()))]),
        BTreeMap::from([(expected.key(), Arc::new(expected.clone()))]),
    )
    .unwrap();
    let cancel = AtomicBool::new(false);
    let resources =
        hex_engine::native_device_resources::PreparedDeviceResources::prepare_on_control(
            &hex_engine::native_device_resources::ResidentDeviceAssets::default(),
            editor.project(),
            1 << 20,
            &cancel,
        )
        .unwrap();
    let ordinary = media
        .prepare_with_device_resources_and_catalog(
            editor.project(),
            Arc::new(hex_engine::SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            0,
            &cancel,
            Some(&catalog),
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        ordinary.arrangement_stream_sources().next().unwrap(),
        &expected
    );
    assert!(catalog.matches_source(&expected));
    let raw_sources = BTreeMap::from([(asset, Arc::new(raw.clone()))]);
    assert!(
        ResidentFileMetadata::new_with_pitched(
            7,
            raw_sources.clone(),
            BTreeMap::from([(expected.key(), Arc::new(expected.clone()))]),
        )
        .is_err()
    );
    let missing = ResidentFileMetadata::new(7, raw_sources.clone()).unwrap();
    assert!(
        media
            .prepare_with_device_resources_and_catalog(
                editor.project(),
                Arc::new(hex_engine::SineTable::new()),
                None,
                &hex_engine::audio_transform::AudioTransformPreparation::default(),
                &resources,
                0,
                &cancel,
                Some(&missing),
                None,
                None,
            )
            .is_err()
    );
    let other_path = files.wav("other-repitch.wav", 16_000, 8_000, 2048);
    let other =
        crate::VerifiedAudioFile::inspect(other_path, 16_000, file_reservation(2).reader, || false)
            .unwrap();
    let foreign = other
        .source_metadata(asset, 7)
        .unwrap()
        .with_repitch(&map, law)
        .unwrap();
    assert!(
        ResidentFileMetadata::new_with_derived(
            7,
            raw_sources.clone(),
            BTreeMap::from([(foreign.key(), Arc::new(foreign))]),
        )
        .is_err()
    );
    let mut alternate = Editor::new(editor.project().clone(), 8).unwrap();
    alternate
        .execute(Command::SetProjectTempo {
            tempo: hex_project::Tempo::new(180_000_000).unwrap(),
        })
        .unwrap();
    let (other_map, other_law) = hex_engine::audio_transform::streamed_repitch_map(
        alternate
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap(),
        alternate.project().tempo_map(),
        raw.playback_frames(),
        &cancel,
    )
    .unwrap();
    let wrong_map = raw.with_repitch(&other_map, other_law).unwrap();
    assert_ne!(wrong_map.key(), expected.key());
    let mismatched = ResidentFileMetadata::new_with_derived(
        7,
        raw_sources,
        BTreeMap::from([(wrong_map.key(), Arc::new(wrong_map))]),
    )
    .unwrap();
    assert!(
        media
            .prepare_with_device_resources_and_catalog(
                editor.project(),
                Arc::new(hex_engine::SineTable::new()),
                None,
                &hex_engine::audio_transform::AudioTransformPreparation::default(),
                &resources,
                0,
                &cancel,
                Some(&mismatched),
                None,
                None,
            )
            .is_err()
    );
    let hosted = hex_engine::AudioCandidateEffects::with_host_definitions(&[], |effects| {
        media.prepare_with_device_resources_and_catalog(
            editor.project(),
            Arc::new(hex_engine::SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            0,
            &cancel,
            Some(&catalog),
            Some(effects),
            None,
        )
    })
    .unwrap();
    assert_eq!(
        hosted.arrangement_stream_sources().next().unwrap(),
        &expected
    );
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 100 })
        .unwrap();
    assert!(
        media
            .prepare(
                editor.project(),
                Arc::new(hex_engine::SineTable::new()),
                None
            )
            .is_err()
    );
}

#[test]
fn ordinary_file_pitch_prepares_only_with_exact_host_metadata() {
    use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
    use hex_project::{Command, Created, Editor, Project, WarpResampler};
    let files = VerifiedFiles::new();
    let path = files.wav("pitch.wav", 16_000, 8192, 1024);
    let other_path = files.wav("other.wav", 16_000, 8192, 2048);
    let reader = file_reservation(2).reader;
    let file = crate::VerifiedAudioFile::inspect(path.clone(), 16_000, reader, || false).unwrap();
    let other = crate::VerifiedAudioFile::inspect(other_path, 16_000, reader, || false).unwrap();
    let mut editor = Editor::new(Project::new(16_000).unwrap(), 8).unwrap();
    let Created::Imported { clip, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "file".into(),
            path,
            sample_rate: 16_000,
            frames: 8192,
        })
        .unwrap()
    else {
        panic!("file fixture")
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 700 })
        .unwrap();
    assert!(matches!(
        editor.execute(Command::DuplicateClip(clip)).unwrap(),
        Created::Clip(_)
    ));
    let raw = file.source_metadata(asset, 7).unwrap();
    let pitched = raw.with_plain_pitch(700, WarpResampler::V2).unwrap();
    let sources = BTreeMap::from([(asset, Arc::new(raw.clone()))]);
    let wrong = other
        .source_metadata(asset, 7)
        .unwrap()
        .with_plain_pitch(700, WarpResampler::V2)
        .unwrap();
    assert!(
        ResidentFileMetadata::new_with_pitched(
            7,
            sources.clone(),
            BTreeMap::from([(pitched.key(), Arc::new(wrong))])
        )
        .is_err()
    );
    assert!(
        ResidentFileMetadata::new_with_pitched(
            7,
            sources.clone(),
            BTreeMap::from([(raw.key(), Arc::new(raw.clone()))])
        )
        .is_err()
    );
    let other_pitch = raw.with_plain_pitch(-300, WarpResampler::V2).unwrap();
    assert!(
        ResidentFileMetadata::new_with_pitched(
            7,
            sources.clone(),
            BTreeMap::from([(pitched.key(), Arc::new(other_pitch))]),
        )
        .is_err()
    );
    let missing = ResidentFileMetadata::new(7, sources.clone()).unwrap();
    let catalog = ResidentFileMetadata::new_with_pitched(
        7,
        sources,
        BTreeMap::from([(pitched.key(), Arc::new(pitched.clone()))]),
    )
    .unwrap();
    let rows = [(asset, &file)];
    let pitches = [ResidentFilePitch {
        asset,
        cents: 700,
        law: WarpResampler::V2,
    }];
    let input = ResidentFileInput {
        context: hex_engine::AudioSourceContext {
            project_generation: 1,
            editor_revision: 1,
        },
        semantic_source_epoch: 1,
        files: &rows,
        pitches: &pitches,
        repitches: &[],
        granulars: &[],
    };
    catalog.validate_for(&input).unwrap();
    assert!(missing.validate_for(&input).is_err());
    assert!(
        catalog
            .validate_for(&ResidentFileInput {
                pitches: &[],
                ..input
            })
            .is_err()
    );
    let supplied = CatalogSink { reject: false }
        .prepare_resident_file_metadata(ResidentFileInput {
            context: input.context,
            semantic_source_epoch: input.semantic_source_epoch,
            files: input.files,
            pitches: input.pitches,
            repitches: input.repitches,
            granulars: &[],
        })
        .unwrap()
        .unwrap();
    supplied.validate_for(&input).unwrap();
    let wrong_rows = [(asset, &other)];
    assert!(
        catalog
            .validate_for(&ResidentFileInput {
                files: &wrong_rows,
                ..input
            })
            .is_err()
    );
    let media = crate::audio_sources::MediaBank {
        epoch: 1,
        device_only: Default::default(),
        resident: BTreeMap::new(),
        verified: BTreeMap::from([(asset, file)]),
    };
    assert_eq!(
        media.required_verified_pitches(editor.project()).unwrap(),
        pitches
    );
    let cancel = AtomicBool::new(false);
    let resources = PreparedDeviceResources::prepare_on_control(
        &ResidentDeviceAssets::default(),
        editor.project(),
        1 << 20,
        &cancel,
    )
    .unwrap();
    let transforms = hex_engine::audio_transform::AudioTransformPreparation::default();
    let prepare = |catalog, effects| {
        media.prepare_with_device_resources_and_catalog(
            editor.project(),
            Arc::new(hex_engine::SineTable::new()),
            None,
            &transforms,
            &resources,
            0,
            &cancel,
            Some(catalog),
            effects,
            None,
        )
    };
    assert!(prepare(&missing, None).is_err());
    let builtins = prepare(&catalog, None).unwrap();
    assert_eq!(
        builtins.arrangement_stream_sources().next().unwrap(),
        &pitched
    );
    let hosted = hex_engine::AudioCandidateEffects::with_host_definitions(&[], |effects| {
        media.prepare_with_device_resources_and_catalog(
            editor.project(),
            Arc::new(hex_engine::SineTable::new()),
            None,
            &transforms,
            &resources,
            0,
            &cancel,
            Some(&catalog),
            Some(effects),
            None,
        )
    })
    .unwrap();
    assert_eq!(
        hosted.arrangement_stream_sources().next().unwrap(),
        &pitched
    );
}

struct CatalogSink {
    reject: bool,
}
impl WorkerAudioSink for CatalogSink {
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        Ok(16_000)
    }
    fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
    fn prepare_resident_file_metadata(
        &mut self,
        input: ResidentFileInput<'_>,
    ) -> Result<Option<ResidentFileMetadata>, String> {
        if self.reject {
            return Err("fixture refusal".into());
        }
        let mut sources = BTreeMap::new();
        for (id, file) in input.files {
            sources.insert(
                *id,
                Arc::new(file.source_metadata(*id, 7).map_err(|e| e.to_string())?),
            );
        }
        let mut pitched = BTreeMap::new();
        for pitch in input.pitches {
            let raw = sources.get(&pitch.asset).ok_or("pitch file missing")?;
            let metadata = raw
                .with_plain_pitch(pitch.cents, pitch.law)
                .map_err(|e| e.to_string())?;
            pitched.insert(metadata.key(), Arc::new(metadata));
        }
        for requirement in input.repitches {
            let raw = sources
                .get(&requirement.asset())
                .ok_or("repitch file missing")?;
            let derived = requirement.metadata_for(raw).map_err(|e| e.to_string())?;
            pitched.insert(derived.key(), Arc::new(derived));
        }
        for requirement in input.granulars {
            let raw = sources
                .get(&requirement.asset())
                .ok_or("granular file missing")?;
            let derived = requirement.metadata_for(raw).map_err(|e| e.to_string())?;
            pitched.insert(derived.key(), Arc::new(derived));
        }
        ResidentFileMetadata::new_with_derived(7, sources, pitched).map(Some)
    }
    fn poll(&mut self) {}
    fn close(&mut self) {}
}

struct DispatchSink {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    expected: hex_engine::AudioSourceContext,
    refuse: bool,
}
impl WorkerAudioSink for DispatchSink {
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        Ok(16_000)
    }
    fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
    fn resident_publication_enabled(&self) -> bool {
        true
    }
    fn prepare_resident_file_metadata(
        &mut self,
        input: ResidentFileInput<'_>,
    ) -> Result<Option<ResidentFileMetadata>, String> {
        CatalogSink { reject: false }.prepare_resident_file_metadata(input)
    }
    fn publish_resident_with_files(
        &mut self,
        playback: hex_engine::playback::Playback,
        resources: Option<&hex_engine::native_device_resources::PreparedDeviceResources>,
        _: Arc<hex_engine::PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        _: usize,
        admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
        files: Option<ResidentFilePublication<'_>>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        assert_eq!(metadata.context, self.expected);
        assert!(matches!(
            policy,
            ResidentPublicationPolicy::ReplaceDocument { .. }
        ));
        assert!(playback.contains_streamed_sources());
        assert!(resources.is_some_and(|resources| resources.metadata_bytes().unwrap() > 0));
        let files = files.unwrap().sources;
        assert_eq!(files.context(), self.expected);
        assert_eq!(files.metadata().catalog_generation(), 7);
        assert_eq!(files.files().len(), 1);
        if self.refuse {
            return Err(ResidentPublicationRefusal {
                kind: super::super::ResidentPublicationFailure::Other,
                playback,
                error: "fixture publication refused".into(),
            });
        }
        if let Err(error) = admit(&[]) {
            return Err(ResidentPublicationRefusal {
                kind: super::super::ResidentPublicationFailure::Other,
                playback,
                error,
            });
        }
        Ok(ResidentPublicationAcceptance::OfflineRetained)
    }
    fn poll(&mut self) {}
    fn close(&mut self) {}
}

#[test]
fn verified_recovered_sampler_and_file_prepare_in_one_bounded_control_graph() {
    let original = sampler_export::tests::Files::new(false);
    let clip_files = VerifiedFiles::new();
    let clip = clip_files.wav("clip.wav", 16_000, 16, 1024);
    let saved = original.source().with_file_name("mixed.hex");
    let mut limits = sampler_export::tests::reservation();
    limits.storage.max_retained_bytes = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 2;
    limits.storage.max_construction_peak_bytes =
        hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 3;
    let service = Service::with_native_reservation(16_000, 16, 1024, limits).unwrap();
    sampler_export::tests::event(&service);
    service.submit(Request::Import(clip)).unwrap();
    let imported = sampler_export::tests::event(&service);
    assert!(imported.error.is_none());
    let file_only_project = imported.state.project.as_ref().clone();
    sampler_export::tests::assign(&service, original.source().to_owned());
    service.submit(Request::SaveNew(saved.clone())).unwrap();
    assert!(sampler_export::tests::event(&service).error.is_none());
    let project = hex_storage::load(&saved).unwrap();
    let old = Session::new(16_000, 16, 1024)
        .unwrap()
        .with_native_reservation(limits)
        .unwrap();
    let candidate = recovery::prepare_project(
        &old,
        16,
        project.clone(),
        saved.parent().unwrap(),
        DocumentAudioMode::VerifiedStreaming,
        Some(file_reservation(2)),
        Some(limits),
        1024,
        sampler_export::Admission::default(),
    )
    .unwrap();
    let before = old.project().snapshot();
    let context = hex_engine::AudioSourceContext {
        project_generation: 3,
        editor_revision: 9,
    };
    let inputs = candidate
        .document_replacement_inputs(&old, sampler_export::Admission::default(), None)
        .unwrap();
    let mut sink = CatalogSink { reject: false };
    let build = |sink: &mut CatalogSink| {
        inputs.prepare(
            candidate.project(),
            sink,
            &candidate.media_bank(),
            candidate.sine_table(),
            &candidate.audio_transforms,
            &candidate.audio_clip_waveforms,
            context,
        )
    };
    let snapshot = build(&mut sink).unwrap();
    assert_eq!(snapshot.prepared_context, context);
    assert!(snapshot.prepared.arrangement().contains_streamed_sources());
    assert!(snapshot.prepared.requires_device_resources());
    assert!(snapshot.resources.metadata_bytes().unwrap() > 0);
    let files = snapshot.file_sources.as_ref().unwrap();
    assert_eq!(files.context(), context);
    assert_eq!(files.metadata().catalog_generation(), 7);
    assert_eq!(files.files().len(), 1);
    let (id, source) = &files.files()[0];
    assert_eq!(
        files.metadata().sources()[id].as_ref(),
        &source.source_metadata(*id, 7).unwrap()
    );
    let clip_id = project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .find(|clip| clip.asset() == *id)
        .unwrap()
        .id();
    let mut editor = hex_project::Editor::new(project.clone(), 8).unwrap();
    editor
        .execute(hex_project::Command::SetClipFades {
            clip: clip_id,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor
        .execute(hex_project::Command::SetAudioClipPitch {
            clip: clip_id,
            cents: 700,
        })
        .unwrap();
    let pitched = inputs
        .prepare(
            editor.project(),
            &mut sink,
            &candidate.media_bank(),
            candidate.sine_table(),
            &candidate.audio_transforms,
            &candidate.audio_clip_waveforms,
            context,
        )
        .unwrap();
    let pitched_files = pitched.file_sources.as_ref().unwrap();
    assert_eq!(pitched_files.pitches().len(), 1);
    let expected = source
        .source_metadata(*id, 7)
        .unwrap()
        .with_plain_pitch(700, hex_project::WarpResampler::V2)
        .unwrap();
    assert!(pitched_files.metadata().matches_source(&expected));
    assert_eq!(
        pitched
            .prepared
            .arrangement_stream_sources()
            .next()
            .unwrap(),
        &expected
    );
    let mut repitched_editor = hex_project::Editor::new(project.clone(), 8).unwrap();
    repitched_editor
        .execute(hex_project::Command::SetClipFades {
            clip: clip_id,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let warp = hex_project::default_audio_warp(
        repitched_editor.project().clip(clip_id).unwrap(),
        repitched_editor.project().tempo_map(),
        hex_project::WarpMode::RePitch,
    )
    .unwrap();
    repitched_editor
        .execute(hex_project::Command::SetAudioClipWarp {
            clip: clip_id,
            settings: Some(warp),
        })
        .unwrap();
    assert!(matches!(
        repitched_editor
            .execute(hex_project::Command::DuplicateClip(clip_id))
            .unwrap(),
        hex_project::Created::Clip(_)
    ));
    let repitched = inputs
        .prepare(
            repitched_editor.project(),
            &mut sink,
            &candidate.media_bank(),
            candidate.sine_table(),
            &candidate.audio_transforms,
            &candidate.audio_clip_waveforms,
            context,
        )
        .unwrap();
    assert!(
        repitched
            .prepared
            .arrangement_stream_sources()
            .next()
            .unwrap()
            .key()
            .repitch_identity()
            .is_some()
    );
    let captured = repitched.file_sources.as_ref().unwrap();
    assert_eq!(captured.repitches().requirements().len(), 1);
    let req = &captured.repitches().requirements()[0];
    let derived = req
        .metadata_for(&source.source_metadata(*id, 7).unwrap())
        .unwrap();
    assert!(captured.metadata().matches_source(&derived));
    let refs = [(*id, source)];
    let input = ResidentFileInput {
        context,
        semantic_source_epoch: captured.semantic_source_epoch(),
        files: &refs,
        pitches: &[],
        repitches: captured.repitches().requirements(),
        granulars: &[],
    };
    captured.metadata().validate_for(&input).unwrap();
    let foreign_path = clip_files.wav("foreign.wav", 16_000, 16, 2048);
    let foreign =
        crate::VerifiedAudioFile::inspect(foreign_path, 16_000, file_reservation(2).reader, || {
            false
        })
        .unwrap();
    let foreign_refs = [(*id, &foreign)];
    assert!(
        captured
            .metadata()
            .validate_for(&ResidentFileInput {
                files: &foreign_refs,
                ..input
            })
            .is_err()
    );
    let mut refusing = CatalogSink { reject: true };
    assert!(
        inputs
            .prepare(
                repitched_editor.project(),
                &mut refusing,
                &candidate.media_bank(),
                candidate.sine_table(),
                &candidate.audio_transforms,
                &candidate.audio_clip_waveforms,
                context,
            )
            .is_err()
    );
    assert!(
        ResidentFileMetadata::new(7, captured.metadata().sources().clone())
            .unwrap()
            .validate_for(&input)
            .is_err()
    );
    let mut changed = hex_project::Editor::new(repitched_editor.project().clone(), 8).unwrap();
    changed
        .execute(hex_project::Command::SetProjectTempo {
            tempo: hex_project::Tempo::new(180_000_000).unwrap(),
        })
        .unwrap();
    let raw = source.source_metadata(*id, 7).unwrap();
    let changed_requirement = hex_engine::audio_transform::StreamedRepitchRequirement::new(
        &raw,
        changed
            .project()
            .clip(clip_id)
            .unwrap()
            .audio_edit()
            .unwrap(),
        changed.project().tempo_map(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let changed_metadata = changed_requirement.metadata_for(&raw).unwrap();
    assert_ne!(changed_metadata.key(), derived.key());
    let changed_only = [changed_requirement];
    assert!(
        captured
            .metadata()
            .validate_for(&ResidentFileInput {
                repitches: &changed_only,
                ..input
            })
            .is_err()
    );
    let two_maps = ResidentFileMetadata::new_with_derived(
        7,
        captured.metadata().sources().clone(),
        BTreeMap::from([
            (derived.key(), Arc::new(derived.clone())),
            (changed_metadata.key(), Arc::new(changed_metadata)),
        ]),
    )
    .unwrap();
    let duplicates = [req.clone(), req.clone()];
    assert!(
        two_maps
            .validate_for(&ResidentFileInput {
                repitches: &duplicates,
                ..input
            })
            .is_err()
    );
    let full = [repitched.admission()];
    let bare = [DeviceResourceAdmission::prepared_on_control(
        &repitched.resources,
        &repitched.prepared,
    )];
    let phase = |registered| DeviceResourcePhase {
        stores: &[],
        registered,
        selections: &[],
        outbound: &[],
        legacy: &[],
        checked_runtime_bytes: 0,
        construction_bytes: 0,
    };
    let full_phase = phase(&full);
    let bare_phase = phase(&bare);
    let full_bytes = hex_engine::native_device_resources::inspect_device_resource_union(
        &full_phase,
        &full_phase,
        limits.storage,
    )
    .unwrap()
    .retained
    .retained_bytes;
    let bare_bytes = hex_engine::native_device_resources::inspect_device_resource_union(
        &bare_phase,
        &bare_phase,
        limits.storage,
    )
    .unwrap()
    .retained
    .retained_bytes;
    assert!(full_bytes >= bare_bytes + req.retained_bytes().unwrap());
    let mut too_small = limits.storage;
    too_small.max_retained_bytes = full_bytes - 1;
    assert!(
        hex_engine::native_device_resources::inspect_device_resource_union(
            &full_phase,
            &full_phase,
            too_small
        )
        .is_err()
    );
    assert_eq!(candidate.project(), &project);
    assert_eq!(old.project().snapshot(), before);
    let mut limited = candidate
        .document_replacement_inputs(&old, sampler_export::Admission::default(), None)
        .unwrap();
    limited.max_frames = 0;
    assert!(
        limited
            .prepare(
                candidate.project(),
                &mut sink,
                &candidate.media_bank(),
                candidate.sine_table(),
                &candidate.audio_transforms,
                &candidate.audio_clip_waveforms,
                context
            )
            .is_err()
    );
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let expected = hex_engine::AudioSourceContext {
        project_generation: 4,
        editor_revision: 10,
    };
    let (sender, _receiver) = std::sync::mpsc::sync_channel(4);
    let mut events = WorkerEvents::new(
        sender,
        Arc::new(AtomicBool::new(false)),
        Some(Box::new(DispatchSink {
            calls: Arc::clone(&calls),
            expected,
            refuse: false,
        })),
    );
    let dispatched = prepare_document_replacement(
        &candidate,
        &old,
        context,
        &mut events,
        None,
        sampler_export::Admission::default(),
    )
    .unwrap();
    assert!(dispatched.resident.is_some());
    assert!(dispatched.sources.is_none());
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(old.project().snapshot(), before);
    let (sender, _receiver) = std::sync::mpsc::sync_channel(4);
    let mut events = WorkerEvents::new(
        sender,
        Arc::new(AtomicBool::new(false)),
        Some(Box::new(DispatchSink {
            calls: Arc::clone(&calls),
            expected,
            refuse: true,
        })),
    );
    assert!(
        prepare_document_replacement(
            &candidate,
            &old,
            context,
            &mut events,
            None,
            sampler_export::Admission::default()
        )
        .is_err()
    );
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(old.project().snapshot(), before);
    let file_only = recovery::prepare_project(
        &old,
        16,
        file_only_project,
        saved.parent().unwrap(),
        DocumentAudioMode::VerifiedStreaming,
        Some(file_reservation(2)),
        Some(limits),
        1024,
        sampler_export::Admission::default(),
    )
    .unwrap();
    let (sender, _receiver) = std::sync::mpsc::sync_channel(4);
    let mut events = WorkerEvents::new(
        sender,
        Arc::new(AtomicBool::new(false)),
        Some(Box::new(DispatchSink {
            calls: Arc::clone(&calls),
            expected,
            refuse: false,
        })),
    );
    let unchanged = prepare_document_replacement(
        &file_only,
        &old,
        context,
        &mut events,
        None,
        sampler_export::Admission::default(),
    )
    .unwrap();
    assert!(unchanged.resident.is_none());
    assert_eq!(
        calls.load(Ordering::Relaxed),
        2,
        "verified file-only projects retain their existing owner"
    );
    assert_eq!(old.project().snapshot(), before);
    sink.reject = true;
    assert!(build(&mut sink).is_err());
    candidate.shutdown.store(true, Ordering::Release);
    sink.reject = false;
    assert!(build(&mut sink).is_err());
    assert_eq!(old.project().snapshot(), before);
}

#[test]
fn ordinary_granular_metadata_capture_rejects_missing_duplicate_stale_and_foreign_facts() {
    use hex_engine::audio_transform::StreamedGranularRequirement;
    use hex_project::{Command, Created, Editor, Project, WarpMode};
    for mode in [
        WarpMode::Tones { grain_ms: 32 },
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        let files = VerifiedFiles::new();
        let path = files.wav("granular.wav", 16_000, 8192, 1024);
        let foreign_path = files.wav("foreign-granular.wav", 16_000, 8192, 2048);
        let file = crate::VerifiedAudioFile::inspect(
            path.clone(),
            16_000,
            file_reservation(2).reader,
            || false,
        )
        .unwrap();
        let foreign = crate::VerifiedAudioFile::inspect(
            foreign_path,
            16_000,
            file_reservation(2).reader,
            || false,
        )
        .unwrap();
        let mut editor = Editor::new(Project::new(16_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "granular".into(),
                path,
                sample_rate: 16_000,
                frames: 8192,
            })
            .unwrap()
        else {
            panic!("fixture")
        };
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        let warp = hex_project::default_audio_warp(
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
        let raw = file.source_metadata(asset, 7).unwrap();
        let requirement = StreamedGranularRequirement::new(
            &raw,
            editor.project().clip(clip).unwrap().audio_edit().unwrap(),
            editor.project().tempo_map(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let metadata = requirement.metadata_for(&raw).unwrap();
        let catalog = ResidentFileMetadata::new_with_derived(
            7,
            BTreeMap::from([(asset, Arc::new(raw.clone()))]),
            BTreeMap::from([(metadata.key(), Arc::new(metadata.clone()))]),
        )
        .unwrap();
        let rows = [(asset, &file)];
        let requirements = [requirement.clone()];
        let input = ResidentFileInput {
            context: hex_engine::AudioSourceContext {
                project_generation: 1,
                editor_revision: 1,
            },
            semantic_source_epoch: 1,
            files: &rows,
            pitches: &[],
            repitches: &[],
            granulars: &requirements,
        };
        catalog.validate_for(&input).unwrap();
        let captured =
            ResidentFileSet::capture(ResidentFileInput { ..input }, catalog.clone()).unwrap();
        assert_eq!(captured.granulars().requirements().len(), 1);
        assert_eq!(
            captured.granulars().requirements()[0]
                .metadata_for(&raw)
                .unwrap(),
            metadata
        );
        let missing = ResidentFileMetadata::new(7, catalog.sources().clone()).unwrap();
        assert!(missing.validate_for(&input).is_err());
        let foreign_rows = [(asset, &foreign)];
        assert!(
            catalog
                .validate_for(&ResidentFileInput {
                    files: &foreign_rows,
                    ..input
                })
                .is_err()
        );
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 300 })
            .unwrap();
        let changed = StreamedGranularRequirement::new(
            &raw,
            editor.project().clip(clip).unwrap().audio_edit().unwrap(),
            editor.project().tempo_map(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let changed_metadata = changed.metadata_for(&raw).unwrap();
        assert_ne!(metadata.key(), changed_metadata.key());
        assert!(
            catalog
                .validate_for(&ResidentFileInput {
                    granulars: &[changed],
                    ..input
                })
                .is_err()
        );
        let two = ResidentFileMetadata::new_with_derived(
            7,
            catalog.sources().clone(),
            BTreeMap::from([
                (metadata.key(), Arc::new(metadata)),
                (changed_metadata.key(), Arc::new(changed_metadata)),
            ]),
        )
        .unwrap();
        assert!(
            two.validate_for(&ResidentFileInput {
                granulars: &[requirement.clone(), requirement.clone()],
                ..input
            })
            .is_err()
        );
        let full = [captured.include_requirements(DeviceResourceAdmission::default())];
        let phase = |registered| DeviceResourcePhase {
            stores: &[],
            registered,
            selections: &[],
            outbound: &[],
            legacy: &[],
            checked_runtime_bytes: 0,
            construction_bytes: 0,
        };
        let limits = hex_engine::AudioStorageLedgerReservation {
            data_rows: 256,
            handle_rows: 256,
            root_rows: 64,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        };
        let inventory = hex_engine::native_device_resources::inspect_device_resource_union(
            &phase(&full),
            &phase(&full),
            limits,
        )
        .unwrap();
        let empty = hex_engine::native_device_resources::inspect_device_resource_union(
            &phase(&[]),
            &phase(&[]),
            limits,
        )
        .unwrap();
        assert!(
            inventory.retained.retained_bytes
                >= empty.retained.retained_bytes + requirement.retained_bytes().unwrap()
        );
        assert_eq!(inventory.retained_frames, 0);
    }
}
