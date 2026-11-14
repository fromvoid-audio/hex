//! Unsupported scalar offline capture refuses before resolving any file.
use super::*;
use hex_project::{Command, Created, Editor, Project};

#[test]
fn pitched_and_raw_file_pages_match_resident_and_validate_source_identity() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(pitched_and_raw_file_pages_case)
        .unwrap()
        .join()
        .unwrap();
}

fn pitched_and_raw_file_pages_case() {
    use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
    use std::collections::BTreeMap;
    const FRAMES: usize = 8192;
    let cancelled = AtomicBool::new(false);
    let dir = std::env::temp_dir().join(format!(
        "hex-pitch-export-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let path = dir.join("source.wav");
    let other_path = dir.join("other.wav");
    let pcm: Vec<_> = (0..FRAMES)
        .map(|n| {
            let sample = ((n as f32) * 0.071).sin() * 0.2;
            [sample, -sample]
        })
        .collect();
    for (target, scale) in [(&path, 1.0), (&other_path, 0.5)] {
        let mut offset = 0;
        hex_export::write_float_wav_new(target, 48_000, FRAMES as u64, &cancelled, |output| {
            let end = offset + output.len();
            for (dst, src) in output.iter_mut().zip(&pcm[offset..end]) {
                *dst = src.map(|x| x * scale);
            }
            offset += output.len();
            Ok(())
        })
        .unwrap();
    }
    let limits = hex_media::VerifiedWavReaderLimits::new(
        hex_media::WavReaderLimits::new(1 << 20, 65_536, 128, 4096),
        1 << 20,
    );
    let file = VerifiedAudioFile::inspect(path.clone(), 48_000, limits, || false).unwrap();
    let other = VerifiedAudioFile::inspect(other_path, 48_000, limits, || false).unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { clip, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: path.clone(),
            sample_rate: 48_000,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture")
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let Created::Clip(pitched_clip) = editor.execute(Command::DuplicateClip(clip)).unwrap() else {
        panic!("duplicate fixture")
    };
    editor
        .execute(Command::SetAudioClipPitch {
            clip: pitched_clip,
            cents: 700,
        })
        .unwrap();
    let raw = file.source_metadata(asset, 7).unwrap();
    let pitched = raw
        .with_plain_pitch(700, hex_project::WarpResampler::V2)
        .unwrap();
    let sources = BTreeMap::from([(
        asset,
        hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(raw.clone())),
    )]);
    let catalog = BTreeMap::from([(pitched.key(), Arc::new(pitched.clone()))]);
    let resources = PreparedDeviceResources::prepare_on_control(
        &ResidentDeviceAssets::default(),
        editor.project(),
        1 << 20,
        &cancelled,
    )
    .unwrap();
    let transforms = hex_engine::audio_transform::AudioTransformPreparation::default();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare_with_prepared_pitch_sources_and_device_resources(
            editor.project(),
            &sources,
            &catalog,
            7,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &transforms,
            &resources,
            0,
            &cancelled,
        )
        .unwrap(),
    );
    let resident = BTreeMap::from([(asset, hex_engine::Audio::new(48_000, pcm).unwrap())]);
    let oracle = Arc::new(
        hex_engine::PreparedRevision::prepare_with_device_resources(
            editor.project(),
            &resident,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &transforms,
            &resources,
            0,
            &cancelled,
        )
        .unwrap(),
    );
    let config = StreamConfig::with_pool_pages(4);
    assert!(
        VerifiedPages::capture_files(&revision, |_| Ok(&file), Some(&path), config, &cancelled)
            .is_err()
    );
    assert!(
        VerifiedPages::capture_files(
            &revision,
            |key| if key.pitch().is_some() {
                Ok(&other)
            } else {
                Ok(&file)
            },
            None,
            config,
            &cancelled
        )
        .is_err()
    );
    let mut pages =
        VerifiedPages::capture_files(&revision, |_| Ok(&file), None, config, &cancelled).unwrap();
    assert_eq!(pages.files.len(), 2);
    for start in [0, FRAMES as u64] {
        let mut actual =
            SessionPlayback::new_with_device_resources(Arc::clone(&revision), &resources).unwrap();
        let mut expected =
            SessionPlayback::new_with_device_resources(Arc::clone(&oracle), &resources).unwrap();
        for playback in [&mut actual, &mut expected] {
            playback
                .apply(hex_engine::SessionPlaybackCommand::Seek(start))
                .unwrap();
            playback
                .apply(hex_engine::SessionPlaybackCommand::Play)
                .unwrap();
        }
        let mut out = [[0.0; 2]; 193];
        let mut reference = [[0.0; 2]; 193];
        assert_eq!(
            hex_engine::PreparedPrerollPages::prepare_chunk(
                &mut pages,
                &actual,
                out.len(),
                &cancelled
            )
            .unwrap(),
            out.len()
        );
        actual.render_with_pages(&mut out, &mut pages).unwrap();
        hex_engine::PreparedPrerollPages::complete_chunk(&mut pages).unwrap();
        expected.render(&mut reference).unwrap();
        assert!(reference.iter().flatten().any(|sample| sample.abs() > 0.01));
        for (actual, expected) in out.iter().flatten().zip(reference.iter().flatten()) {
            assert!((actual - expected).abs() <= 1e-5, "{actual} != {expected}");
        }
    }
    pages.finish_pages(&cancelled).unwrap();
    std::fs::write(&path, b"changed source").unwrap();
    assert!(pages.finish_pages(&cancelled).is_err());
}

#[test]
fn scalar_offline_capture_refuses_before_source_resolution() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "parameter".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
                "hex.utility",
            ))
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    let source = hex_engine::ScalarSourceMetadata::new(
        hex_engine::ScalarSourceKey::new(1, 1, 1).unwrap(),
        hex_engine::ScalarSourceTarget::Device {
            device,
            target: hex_project::DeviceAutomationTarget::HostInputGain,
        },
        48_000,
        0,
        4,
        hex_engine::ScalarProjectionIdentity {
            version: hex_project::AUTOMATION_PROJECTION_VERSION,
            fingerprint: hex_storage::automation_draft::automation_projection_fingerprint(
                editor.project().tempo_map(),
            ),
        },
        hex_engine::ScalarFileIdentity {
            format_version: 2,
            file_bytes: 100,
            terminal_chain: [1; 32],
        },
    )
    .unwrap();
    let prepared = hex_engine::PreparedRevision::prepare_with_scalar_automation(
        editor.project(),
        &std::collections::BTreeMap::new(),
        Arc::new(hex_engine::SineTable::new()),
        None,
        hex_engine::ScalarAutomationBinding {
            device,
            target: hex_project::DeviceAutomationTarget::HostInputGain,
            source,
        },
    )
    .unwrap();
    let reads = std::cell::Cell::new(0);
    let result = VerifiedPages::capture_files(
        &prepared,
        |_| {
            reads.set(reads.get() + 1);
            Err(ExportError::Invalid("unexpected source resolution"))
        },
        None,
        StreamConfig::with_pool_pages(3),
        &AtomicBool::new(false),
    );
    assert_eq!(reads.get(), 0);
    match result {
        Err(ExportError::Invalid(reason)) => {
            assert_eq!(reason, "offline scalar source capture is not prepared")
        }
        _ => panic!("scalar capture must refuse explicitly"),
    }
}

#[test]
fn history_seek_replays_verified_pages_and_effect_history() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(history_seek_case)
        .unwrap()
        .join()
        .unwrap();
}

fn history_seek_case() {
    use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
    use hex_session::arrangement_seek::{ArrangementSeekMode, ArrangementSeekSpec};
    use hex_session::device_gesture::{ParameterId, ParameterValue};
    use std::collections::BTreeMap;
    let cancelled = AtomicBool::new(false);
    let path = std::env::temp_dir().join(format!("hex-history-pages-{}.wav", std::process::id()));
    struct RemoveFile(std::path::PathBuf);
    impl Drop for RemoveFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let frames = 32_768;
    let pcm = vec![[0.25, -0.125]; frames];
    let mut offset = 0;
    hex_export::write_float_wav_new(&path, 48_000, frames as u64, &cancelled, |output| {
        output.copy_from_slice(&pcm[offset..offset + output.len()]);
        offset += output.len();
        Ok(())
    })
    .unwrap();
    let _file = RemoveFile(path.clone());
    let file = VerifiedAudioFile::inspect(
        path.clone(),
        48_000,
        hex_media::VerifiedWavReaderLimits::new(
            hex_media::WavReaderLimits::new(1 << 20, 65_536, 128, 4096),
            1 << 20,
        ),
        || false,
    )
    .unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "verified".into(),
            path: path.clone(),
            sample_rate: 48_000,
            frames: frames as u64,
        })
        .unwrap()
    else {
        panic!("audio import")
    };
    let filter = hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
        "hex.utility",
    ))
    .unwrap();
    let filter = hex_engine::patch_native_device_parameter(
        &filter,
        ParameterId::new(11).unwrap(),
        ParameterValue::Bool(true),
    )
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: filter.clone(),
        })
        .unwrap();
    let Created::ReturnTrack(return_track) = editor
        .execute(Command::AddReturnTrack {
            name: "tail".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    editor
        .execute(Command::InsertReturnDevice {
            return_track,
            before: None,
            state: filter,
        })
        .unwrap();
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track,
            gain: 0.5,
        })
        .unwrap();
    let instrument = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        hex_engine::DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    let instrument = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        &instrument,
        ParameterId::new(78).unwrap(),
        ParameterValue::Float(0.3),
    )
    .unwrap();
    let Created::Track(midi) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "feedback".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: instrument,
        })
        .unwrap()
    else {
        panic!("instrument track")
    };
    let Created::Clip(midi_clip) = editor
        .execute(Command::PlaceMidiClip {
            track: midi,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: midi_clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let project = editor.project();
    let resources = PreparedDeviceResources::prepare_on_control(
        &ResidentDeviceAssets::default(),
        project,
        1 << 20,
        &cancelled,
    )
    .unwrap();
    let sources = BTreeMap::from([(
        asset,
        hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(
            file.source_metadata(asset, 1).unwrap(),
        )),
    )]);
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare_with_prepared_audio_sources_and_device_resources(
            project,
            &sources,
            1,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            0,
            &cancelled,
        )
        .unwrap(),
    );
    let resident = BTreeMap::from([(asset, hex_engine::Audio::new(48_000, pcm).unwrap())]);
    let oracle_revision = Arc::new(
        hex_engine::PreparedRevision::prepare_with_device_resources(
            project,
            &resident,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            0,
            &cancelled,
        )
        .unwrap(),
    );
    for (destination, playing) in [(512, true), (24_004, false)] {
        let mut oracle =
            SessionPlayback::new_with_device_resources(Arc::clone(&oracle_revision), &resources)
                .unwrap();
        oracle
            .apply(hex_engine::SessionPlaybackCommand::Play)
            .unwrap();
        for chunk in vec![[0.0; 2]; destination].chunks_mut(127) {
            oracle.render(chunk).unwrap();
        }
        let mut expected = [[0.0; 2]; 193];
        oracle.render(&mut expected).unwrap();
        let spec = ArrangementSeekSpec {
            mode: ArrangementSeekMode::HistoryPreservingInteractive,
            generation: 7,
            revision: 17,
            prepared_revision: Arc::clone(&revision),
            destination_frame: destination as u64,
            work_budget_frames: destination as u64,
            start_playing: playing,
        };
        let mut actual = prepare_file_seek_runtime(
            &spec,
            &resources,
            hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 17,
            },
            |_| Ok(&file),
            &cancelled,
        )
        .unwrap();
        if !playing {
            actual
                .apply(hex_engine::SessionPlaybackCommand::Play)
                .unwrap();
        }
        let mut pages = VerifiedPages::capture_files(
            &revision,
            |_| Ok(&file),
            None,
            StreamConfig::with_pool_pages(3),
            &cancelled,
        )
        .unwrap();
        let mut rendered = [[0.0; 2]; 193];
        let count =
            hex_engine::PreparedPrerollPages::prepare_chunk(&mut pages, &actual, 193, &cancelled)
                .unwrap();
        assert_eq!(count, 193);
        actual.render_with_pages(&mut rendered, &mut pages).unwrap();
        hex_engine::PreparedPrerollPages::complete_chunk(&mut pages).unwrap();
        pages.finish_pages(&cancelled).unwrap();
        assert_eq!(
            rendered, expected,
            "page-replayed feedback and Track/Return history at {destination}"
        );
    }

    let spec = ArrangementSeekSpec {
        mode: ArrangementSeekMode::HistoryPreservingInteractive,
        generation: 7,
        revision: 17,
        prepared_revision: Arc::clone(&revision),
        destination_frame: 512,
        work_budget_frames: 512,
        start_playing: true,
    };
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 17,
    };
    let resolutions = std::cell::Cell::new(0);
    assert!(
        prepare_file_seek_runtime(
            &spec,
            &resources,
            hex_engine::AudioSourceContext {
                project_generation: 8,
                ..context
            },
            |_| {
                resolutions.set(resolutions.get() + 1);
                Ok(&file)
            },
            &cancelled
        )
        .is_err()
    );
    assert_eq!(resolutions.get(), 0);
    assert!(matches!(
        prepare_file_seek_runtime(
            &spec,
            &resources,
            context,
            |_| Ok(&file),
            &AtomicBool::new(true)
        ),
        Err(hex_engine::ArrangementPrerollError::Cancelled)
    ));
    assert!(
        prepare_file_seek_runtime(
            &spec,
            &resources,
            context,
            |_| Err(ExportError::Invalid("fixture binding unavailable")),
            &cancelled
        )
        .is_err()
    );
    // Captured facts never refresh after mutation. Refusal returns no candidate,
    // and the private page owner joins during control-side error unwinding.
    std::fs::write(&path, b"changed fixture").unwrap();
    assert!(
        prepare_file_seek_runtime(&spec, &resources, context, |_| Ok(&file), &cancelled).is_err()
    );
}
