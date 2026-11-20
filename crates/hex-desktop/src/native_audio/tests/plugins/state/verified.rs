//! Open an actual saved hosted project through the verified constructor.
use super::*;
use hex_project::Editor;
use hex_session::service::ProjectSource;

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn saved_vst3_reopens_and_renders_through_the_verified_output() {
    let _native = native_fixture();
    let saved = nondefault_fixture();
    let mut editor = Editor::new(saved.project.clone(), 8).unwrap();
    let track = editor.project().tracks()[0].id();
    let eq = hex_engine::eq::EqState::default().encode().unwrap();
    let eq = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Effect,
        &eq,
        hex_engine::eq::OUTPUT_GAIN,
        hex_session::device_gesture::ParameterValue::Float(0.5),
    )
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: eq,
        })
        .unwrap();
    let path = saved.files.0.join("verified-hosted.hex");
    hex_storage::save_new(&path, editor.project()).unwrap();
    let original = fs::read(&path).unwrap();
    let progress = Arc::new(Mutex::new(
        crate::native_audio::plugins::PreparationObservation::default(),
    ));
    let queued = Arc::new(AtomicBool::new(false));
    let mut h = super::super::super::verified_gain::native_harness_with(|sink| {
        sink.plugins = crate::native_audio::plugins::Plugins::with_fixture(
            host(),
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
        );
        sink.plugins.catalog = Some(saved.catalog.clone());
        sink.plugins.progress = Some(Arc::clone(&progress));
        sink.plugins.queued = Some(Arc::clone(&queued));
    });
    h.next();
    h.open();
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let open_saved = || Request::OpenWithAudioMode {
        source: ProjectSource::Saved(path.clone()),
        relinks: Default::default(),
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    };
    let opened = h.run(open_saved());
    assert_eq!(opened.state.project.as_ref(), editor.project());
    assert!(
        opened
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .required_sources()
            .any(|(_, source)| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)))
    );
    h.open();
    let applied = h.applied(h.context.unwrap());
    assert_ne!(applied.output_identity, old_output);
    wait(|| progress.lock().unwrap().source_bindings[0].is_some());
    let (device, binding, _) = progress.lock().unwrap().source_bindings[0].unwrap();
    assert_eq!(device, saved.project.main_devices()[0].id().value());
    assert_ne!(binding.root, saved.old_output);
    assert_eq!(binding.root, applied.output_identity);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for quantum in 0..3 {
        let mut pcm = [[0.0; 2]; 256];
        let mut host = [0.0; 512];
        let status = h.audio.as_ref().unwrap().status().unwrap();
        wait(|| {
            let mut backend = h.backend.lock().unwrap();
            let measured = allocation_counter::measure(|| {
                backend
                    .as_mut()
                    .unwrap()
                    .render_native_direct(&mut pcm, &mut host)
                    .unwrap();
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.count_current,
                    measured.bytes_total,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            let advanced = status.performance_frame.load(Acquire) == (quantum + 1) * 256;
            if !advanced {
                std::thread::sleep(Duration::from_millis(1));
            }
            advanced
        });
        wait(|| {
            progress.lock().unwrap().source_bindings[0].is_some_and(|(_, original, count)| {
                original == binding && count.is_some_and(|count| count > quantum)
            })
        });
        assert_eq!(pcm[255], [if quantum == 2 { 0.046875 } else { 0.0 }; 2]);
    }
    let hidden_catalog = saved.files.0.join("catalog-away.json");
    fs::rename(&saved.catalog, &hidden_catalog).unwrap();
    let reopened = h.run(open_saved());
    assert_eq!(reopened.state.project.as_ref(), editor.project());
    h.open();
    let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    assert_ne!(output, applied.output_identity);
    let context = h.context.unwrap();
    wait(|| {
        h.render();
        h.events.try_iter().any(|event| {
            matches!(event, EditorAudioEvent::Publication {
                context: actual, output_identity,
                state: EditorPublicationState::Refused(reason), ..
            } if actual == context && output_identity == output && reason.contains("vst3"))
        })
    });
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
            .is_none()
    );

    fs::rename(hidden_catalog, &saved.catalog).unwrap();
    h.run(open_saved());
    h.open();
    let cancelled = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    wait(|| queued.load(Acquire) && progress.lock().unwrap().output == Some(cancelled));
    // Replace the document while the fresh endpoint waits for the callback
    // to take its loan. No restored graph can install on the new output yet.
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
            .is_none()
    );
    let empty = h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    assert!(empty.state.project.main_devices().is_empty());
    h.open();
    assert_ne!(h.applied(h.context.unwrap()).output_identity, cancelled);
    assert_eq!(fs::read(path).unwrap(), original);
}
