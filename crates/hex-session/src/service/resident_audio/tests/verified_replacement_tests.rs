//! Mixed verified file and original resources on the ordinary control preparation path.
use super::*;
use crate::service::verified_source_tests::{
    Files as VerifiedFiles, reservation as file_reservation,
};

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
        ResidentFileMetadata::new(7, sources).map(Some)
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
