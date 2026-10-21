use super::*;
#[test]
fn current_sampler_console_templates_construct_with_paired_storage() {
    let (project, asset) = fixture();
    let track = project.tracks()[0].id();
    let mut editor = Editor::new(project, 16).unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "console".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "sample".into(),
            source_length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let project = editor.project();
    let quote = AudioCandidateReservationQuote::for_project(project, 1).unwrap();
    let limits = quote.storage_reservation(65536).unwrap();
    let mut cell = AudioCandidatePreparation::try_new_on_control(1, limits).unwrap();
    let source = crate::OwnedAudioSource::try_resident(
        8000,
        vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]],
    )
    .ok()
    .unwrap();
    let original = crate::OwnedAudioInputs::try_new(vec![(asset, source)], limits)
        .ok()
        .unwrap();
    let token = original.source_token(asset).unwrap();
    cell.copy_original_source_on_control(asset, original.source_view(&token).unwrap())
        .unwrap();
    cell.prepare_current_sampler_on_control(
        project,
        11,
        crate::AudioSourceContext {
            project_generation: 4,
            editor_revision: 7,
        },
        4,
        &AtomicBool::new(false),
    )
    .unwrap_or_else(|e| {
        panic!(
            "{e:?}; custody={:?}/{:?}",
            cell.custody.as_ref().map(|c| c.phase()),
            cell.custody.as_ref().and_then(|c| c.reason())
        )
    });
    let owner = cell.owner_on_control().unwrap();
    owner
        .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
            data_rows: quote.data_rows,
            handle_rows: quote.handle_rows,
            root_rows: 32,
            max_retained_bytes: limits.max_prepared_data_bytes,
            max_construction_peak_bytes: limits.max_construction_data_bytes,
        })
        .unwrap();
    assert!(
        owner
            .inner
            .runtime
            .as_ref()
            .unwrap()
            .sampler_console_available()
    );
    owner
        .test_apply_action(crate::PlaybackAction::LaunchAndPlay(
            crate::SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: crate::LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    let mut output = [[0.0; 2]; 12];
    owner.test_render(&mut output).unwrap();
    assert_eq!(
        &output[..6],
        &[
            [0.125, -0.25],
            [0.3125, -0.125],
            [0.5, 0.0],
            [0.1875, 0.125],
            [-0.125, 0.25],
            [-0.0625, 0.125]
        ]
    );
    owner
        .test_apply_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::Stop,
        ))
        .unwrap();
    owner.test_render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 12]);
    cell.cleanup_on_control().unwrap();
}

fn mixed_fixture() -> (
    Project,
    Vec<(hex_project::AssetId, crate::OwnedAudioSource)>,
    hex_project::SceneId,
    hex_project::SceneId,
    hex_project::ClipId,
) {
    let (project, asset) = fixture();
    let first = project.tracks()[0].id();
    let device = project.tracks()[0].instrument().unwrap().id();
    let mut editor = Editor::new(project, 64).unwrap();
    let mut sampler = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
    sampler.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 2.0 / 16000.0,
    };
    let state = sampler
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: state.clone(),
        })
        .unwrap();
    let Created::Track(second) = editor
        .execute(Command::AddMidiTrack {
            name: "second sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track: second,
            state,
        })
        .unwrap();
    editor
        .execute(Command::Gain {
            track: second,
            gain: 0.5,
        })
        .unwrap();
    let Created::Track(audio) = editor
        .execute(Command::AddTrack {
            name: "ordinary audio".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Asset(audio_asset) = editor
        .execute(Command::AddAsset {
            path: "ordinary.wav".into(),
            sample_rate: 16000,
            frames: 16,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Scene(a) = editor
        .execute(Command::AddScene {
            name: "mixed loop".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::Scene(b) = editor
        .execute(Command::AddScene {
            name: "next".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let mut first_clip = None;
    for (track, scene, pitch, looping) in [
        (first, a, 60, true),
        (second, a, 72, true),
        (first, b, 60, false),
    ] {
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "pattern".into(),
                source_length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        if first_clip.is_none() {
            first_clip = Some(clip)
        }
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(BeatTime::ONE.raw() / 2000),
                pitch,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
        if looping {
            editor
                .execute(Command::SetSessionMidiClipSource {
                    clip,
                    source_length: BeatTime::ONE,
                    source_start: BeatTime::ZERO,
                    loop_start: BeatTime::ZERO,
                    loop_end: BeatTime::from_raw(BeatTime::ONE.raw() / 1000),
                    looping: true,
                })
                .unwrap();
        }
    }
    editor
        .execute(Command::CreateSessionAudioClip {
            track: audio,
            scene: a,
            name: "audio".into(),
            asset: audio_asset,
            source_start: 0,
            source_end: 16,
        })
        .unwrap();
    editor
        .execute(Command::SetSessionStopCell {
            track: second,
            scene: b,
        })
        .unwrap();
    editor
        .execute(Command::SetSessionStopCell {
            track: audio,
            scene: b,
        })
        .unwrap();
    let source = crate::OwnedAudioSource::try_resident(
        8000,
        vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]],
    )
    .ok()
    .unwrap();
    let audio = crate::OwnedAudioSource::try_resident(16000, vec![[0.03125; 2]; 16])
        .ok()
        .unwrap();
    (
        editor.project().clone(),
        vec![(asset, source), (audio_asset, audio)],
        a,
        b,
        first_clip.unwrap(),
    )
}
fn prepare(
    project: &Project,
    sources: Vec<(hex_project::AssetId, crate::OwnedAudioSource)>,
) -> AudioCandidatePreparation {
    let quote = AudioCandidateReservationQuote::for_project(project, sources.len()).unwrap();
    let limits = quote.storage_reservation(65536).unwrap();
    let work = sources.iter().map(|(_, s)| s.frames()).sum();
    let original = crate::OwnedAudioInputs::try_new(sources, limits)
        .ok()
        .unwrap();
    let mut cell =
        AudioCandidatePreparation::try_new_on_control(original.sources.len(), limits).unwrap();
    for (asset, _) in &original.sources {
        let token = original.source_token(*asset).unwrap();
        cell.copy_original_source_on_control(*asset, original.source_view(&token).unwrap())
            .unwrap();
    }
    cell.prepare_current_sampler_on_control(
        project,
        11,
        crate::AudioSourceContext {
            project_generation: 4,
            editor_revision: 7,
        },
        work,
        &AtomicBool::new(false),
    )
    .unwrap_or_else(|e| panic!("{e:?}"));
    cell.owner_on_control()
        .unwrap()
        .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
            data_rows: quote.data_rows,
            handle_rows: quote.handle_rows,
            root_rows: 64,
            max_retained_bytes: limits.max_prepared_data_bytes,
            max_construction_peak_bytes: limits.max_construction_data_bytes,
        })
        .unwrap();
    cell
}
#[test]
fn current_sampler_console_mixed_scene_loops_have_literal_pcm_and_partition_invariance() {
    let mut results = Vec::new();
    for partitions in [&[16_usize][..], &[1, 3, 5, 7][..]] {
        let (project, sources, a, b, _) = mixed_fixture();
        let mut cell = prepare(&project, sources);
        let owner = cell.owner_on_control().unwrap();
        owner
            .test_apply_action(crate::PlaybackAction::LaunchAndPlay(
                crate::SessionPlaybackCommand::LaunchScene {
                    scene: a,
                    quantization: crate::LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
        let mut pcm = Vec::new();
        for count in partitions {
            let mut block = vec![[0.0; 2]; *count];
            owner.test_render(&mut block).unwrap();
            pcm.extend(block);
        }
        let expected = [
            [0.21875, -0.34375],
            [0.59375, -0.09375],
            [0.46875, 0.15625],
            [0.21875, 0.15625],
            [-0.09375, 0.28125],
            [0.0, 0.09375],
            [0.03125, 0.03125],
            [0.03125, 0.03125],
        ];
        assert_eq!(&pcm[..8], &expected);
        assert_eq!(&pcm[8..16], &expected);
        owner
            .test_apply_action(crate::PlaybackAction::LaunchAndPlay(
                crate::SessionPlaybackCommand::LaunchScene {
                    scene: b,
                    quantization: crate::LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
        let mut next = [[0.0; 2]; 8];
        owner.test_render(&mut next).unwrap();
        assert_eq!(
            next,
            [
                [0.125, -0.25],
                [0.3125, -0.125],
                [0.5, 0.0],
                [0.1875, 0.125],
                [-0.125, 0.25],
                [-0.03125, 0.0625],
                [0.0; 2],
                [0.0; 2]
            ]
        );
        results.push(pcm);
        cell.cleanup_on_control().unwrap();
    }
    assert_eq!(results[0], results[1]);
}
