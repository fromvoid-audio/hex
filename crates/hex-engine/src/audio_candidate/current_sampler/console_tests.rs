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
    let storage = crate::AudioStorageLedgerReservation {
        data_rows: quote.data_rows,
        handle_rows: quote.handle_rows,
        root_rows: 32,
        max_retained_bytes: limits.max_prepared_data_bytes,
        max_construction_peak_bytes: limits.max_construction_data_bytes,
    };
    let summary = owner.seal_constructor_on_control(storage).unwrap();
    let exact = crate::AudioStorageLedgerReservation {
        max_retained_bytes: summary.data.retained_bytes + summary.reserved_split_bytes,
        max_construction_peak_bytes: summary.data.construction_peak_bytes
            + summary.reserved_split_bytes,
        ..storage
    };
    assert!(
        owner
            .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                max_retained_bytes: exact.max_retained_bytes - 1,
                ..exact
            })
            .is_err()
    );
    assert!(
        owner
            .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                max_construction_peak_bytes: exact.max_construction_peak_bytes - 1,
                ..exact
            })
            .is_err()
    );
    owner.seal_constructor_on_control(exact).unwrap();
    let config = &owner
        .inner
        .runtime
        .as_ref()
        .unwrap()
        .sampler_sources
        .as_ref()
        .unwrap()
        .0
        .metadata
        .configurations()[0];
    let native = project.track(track).unwrap().instrument().unwrap().state();
    assert!(config.matches_state(native));
    assert!(!config.matches_state(&native.clone().with_input_gain_db(-6.0).unwrap()));
    assert!(!config.matches_state(&native.clone().with_output_gain_db(-6.0).unwrap()));
    // The captured saved version is part of the exact configuration.
    let other_version = hex_lattice_contract::NativeDeviceState::new(
        native.type_id(),
        native.state_version() - 1,
        native.payload().to_vec(),
    )
    .unwrap()
    .with_host_metadata_from(native)
    .unwrap();
    assert!(!config.matches_state(&other_version));
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

/// Prepared gated release of the four-frame fixture sampler at the 16 kHz
/// console rate. Literal-PCM oracles derive from it, so a host declick floor
/// changes only how long the expected release lasts.
fn release_frames(release_seconds: f64) -> u64 {
    let mut state = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
    state.envelope.release_seconds = release_seconds;
    hex_devices::sampler::PreparedSamplerPlan::new(
        hex_devices::sampler::SampleShape::new(
            4,
            8000,
            hex_devices::sampler::SampleChannels::Stereo,
        )
        .unwrap(),
        16000,
        state.settings(),
    )
    .unwrap()
    .prepare_voice_coefficients(60, 127)
    .unwrap()
    .unwrap()
    .release_frames()
}

/// Linear release level `steps` frames after note-off.
fn release_level(steps: u64, release: u64) -> f64 {
    if steps >= release {
        0.0
    } else {
        1.0 - steps as f64 / release as f64
    }
}

fn assert_pcm_near(actual: &[[f32; 2]], expected: &[[f64; 2]]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for channel in 0..2 {
            assert!(
                (f64::from(actual[channel]) - expected[channel]).abs() < 1e-6,
                "frame {index}: {actual:?} != {expected:?}"
            );
        }
    }
}

/// Fixture source at pitch 60: four 8 kHz frames read at half speed. The last
/// entry is the looping case, interpolating from the end into the loop start.
const HALF_SPEED: [[f64; 2]; 8] = [
    [0.125, -0.25],
    [0.3125, -0.125],
    [0.5, 0.0],
    [0.1875, 0.125],
    [-0.125, 0.25],
    [-0.0625, 0.125],
    [0.0, 0.0],
    [0.0625, -0.125],
];

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
    try_prepare(project, sources).unwrap_or_else(|e| panic!("{e}"))
}

/// `prepare`, returning the CURRENT preparation refusal instead of panicking.
fn try_prepare(
    project: &Project,
    sources: Vec<(hex_project::AssetId, crate::OwnedAudioSource)>,
) -> Result<AudioCandidatePreparation, String> {
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
    .map_err(|e| format!("{e:?}"))?;
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
    Ok(cell)
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
        // Each four-frame note releases from gate4 until the half-speed
        // source ends at age8; the second track's octave voice ends at age4.
        let after_off = release_level(1, release_frames(2.0 / 16000.0));
        let expected = [
            [0.21875, -0.34375],
            [0.59375, -0.09375],
            [0.46875, 0.15625],
            [0.21875, 0.15625],
            [-0.09375, 0.28125],
            [-0.0625 * after_off + 0.03125, 0.125 * after_off + 0.03125],
            [0.03125, 0.03125],
            [0.03125, 0.03125],
        ];
        assert_pcm_near(&pcm[..8], &expected);
        assert_pcm_near(&pcm[8..16], &expected);
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
        assert_pcm_near(
            &next,
            &[
                [0.125, -0.25],
                [0.3125, -0.125],
                [0.5, 0.0],
                [0.1875, 0.125],
                [-0.125, 0.25],
                [-0.0625 * after_off, 0.125 * after_off],
                [0.0; 2],
                [0.0; 2],
            ],
        );
        results.push(pcm);
        cell.cleanup_on_control().unwrap();
    }
    assert_eq!(results[0], results[1]);
}

#[test]
fn current_sampler_console_retrigger_keeps_old_release_and_uses_a_new_occurrence() {
    let (project, sources, _, _, clip) = mixed_fixture();
    let mut cell = prepare(&project, sources);
    let owner = cell.owner_on_control().unwrap();
    let launch = crate::PlaybackAction::LaunchAndPlay(crate::SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: crate::LaunchQuantization::Immediate,
    });
    owner.test_apply_action(launch).unwrap();
    let mut first = [[0.0; 2]; 1];
    owner.test_render(&mut first).unwrap();
    assert_eq!(first, [[0.125, -0.25]]);
    owner.test_apply_action(launch).unwrap();
    // Cancel must not erase the already committed immediate cut's first-frame
    // event bound. It only removes pending requests.
    owner
        .test_apply_action(crate::PlaybackAction::Session(
            crate::SessionPlaybackCommand::CancelPending { track: None },
        ))
        .unwrap();
    let mut pcm = [[0.0; 2]; 3];
    owner.test_render(&mut pcm).unwrap();
    // The old occurrence releases from its gate at frame1 while the new
    // occurrence starts at age0 on the same frame.
    let release = release_frames(2.0 / 16000.0);
    let expected: Vec<_> = (0..3)
        .map(|step| {
            let old = HALF_SPEED[step + 1];
            let level = release_level(step as u64, release);
            let new = HALF_SPEED[step];
            [old[0] * level + new[0], old[1] * level + new[1]]
        })
        .collect();
    assert_pcm_near(&pcm, &expected);
    cell.cleanup_on_control().unwrap();
}

#[test]
fn current_sampler_console_quantized_scene_cut_matches_immediate_cut_at_boundary() {
    let mut results = Vec::new();
    for quantized in [false, true] {
        let (project, sources, a, b, _) = mixed_fixture();
        let mut cell = prepare(&project, sources);
        let owner = cell.owner_on_control().unwrap();
        let scene = |scene, quantization| {
            crate::PlaybackAction::Session(crate::SessionPlaybackCommand::LaunchScene {
                scene,
                quantization,
            })
        };
        owner
            .test_apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Play,
            ))
            .unwrap();
        owner
            .test_apply_action(scene(a, crate::LaunchQuantization::Immediate))
            .unwrap();
        owner.test_render(&mut [[0.0; 2]; 1]).unwrap();
        if quantized {
            owner
                .test_apply_action(scene(b, crate::LaunchQuantization::FourQuarter))
                .unwrap();
        }
        // 120 bpm at 16 kHz: the next four-quarter boundary is frame 32000.
        let mut remaining = 31998;
        while remaining != 0 {
            let count = remaining.min(64);
            owner.test_render(&mut [[0.0; 2]; 64][..count]).unwrap();
            remaining -= count;
        }
        let mut pcm = Vec::new();
        if quantized {
            let mut block = [[0.0; 2]; 9];
            owner.test_render(&mut block).unwrap();
            pcm.extend(block);
        } else {
            let mut before = [[0.0; 2]; 1];
            owner.test_render(&mut before).unwrap();
            pcm.extend(before);
            owner
                .test_apply_action(scene(b, crate::LaunchQuantization::Immediate))
                .unwrap();
            let mut after = [[0.0; 2]; 8];
            owner.test_render(&mut after).unwrap();
            pcm.extend(after);
        }
        assert_eq!(pcm[1], [0.125, -0.25]);
        results.push(pcm);
        cell.cleanup_on_control().unwrap();
    }
    assert_eq!(results[0], results[1]);
}

#[test]
fn current_sampler_console_one_voice_scene_relaunch_steals_the_release() {
    // The second track's sampler declares stealing. With one voice, its old
    // release and the incoming voice no longer refuse the scene: the release
    // is stolen into the declick pool. Two voices are the no-steal oracle.
    let mut results = Vec::new();
    for voices in [2, 1] {
        let (project, sources, a, _, _) = mixed_fixture();
        let second = project.tracks()[1].instrument().unwrap();
        let mut state =
            hex_devices::sampler::state::SamplerState::from_native(second.state()).unwrap();
        state.voices = voices;
        let native = state
            .to_native()
            .unwrap()
            .with_asset_references(second.state().asset_references().to_vec())
            .unwrap();
        let mut editor = Editor::new(project.clone(), 4).unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: second.id(),
                state: native,
            })
            .unwrap();
        let mut cell = prepare(editor.project(), sources);
        let owner = cell.owner_on_control().unwrap();
        let scene = crate::PlaybackAction::Session(crate::SessionPlaybackCommand::LaunchScene {
            scene: a,
            quantization: crate::LaunchQuantization::Immediate,
        });
        owner
            .test_apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Play,
            ))
            .unwrap();
        owner.test_apply_action(scene).unwrap();
        owner.test_render(&mut [[0.0; 2]; 1]).unwrap();
        owner.test_apply_action(scene).unwrap();
        let mut pcm = [[0.0; 2]; 16];
        owner.test_render(&mut pcm[..1]).unwrap();
        assert_eq!(
            owner
                .inner
                .runtime
                .as_ref()
                .unwrap()
                .playback
                .test_current_sampler_route_proofs(1),
            Some((2, 1, 1)),
            "the old proof keeps its quiet deadline beside the new held voice"
        );
        owner.test_render(&mut pcm[1..]).unwrap();
        results.push(pcm);
        cell.cleanup_on_control().unwrap();
    }
    let (oracle, stolen) = (results[0], results[1]);
    assert_eq!(stolen[0], oracle[0], "the declick starts at unity");
    assert_ne!(stolen[1], oracle[1], "the stolen release is declicked");
    for channel in 0..2 {
        assert!((stolen[1][channel] - oracle[1][channel]).abs() <= 1.0 / 16.0);
    }
    assert_eq!(
        stolen[2..],
        oracle[2..],
        "the stolen voice retires with its own two-frame release"
    );
}

#[test]
fn current_sampler_console_held_note_crosses_quantized_cut_with_literal_release_and_new_voice() {
    let mut results = Vec::new();
    for partitions in [&[8_usize][..], &[3, 1, 4][..], &[1, 1, 1, 2, 3][..]] {
        let (project, asset) = fixture();
        let track = project.tracks()[0].id();
        let device = project.tracks()[0].instrument().unwrap();
        let mut state = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
        state.mode = hex_devices::sampler::state::AuthoredMode::Classic {
            loop_enabled: true,
            loop_region: hex_devices::sampler::Region { start: 0, end: 4 },
        };
        state.envelope = hex_devices::sampler::Envelope {
            attack_seconds: 0.0,
            decay_seconds: 0.0,
            sustain: 1.0,
            release_seconds: 2.0 / 16000.0,
        };
        let native = state
            .to_native()
            .unwrap()
            .with_asset_references(device.state().asset_references().to_vec())
            .unwrap();
        let mut editor = Editor::new(project.clone(), 16).unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: device.id(),
                state: native,
            })
            .unwrap();
        let mut clips = Vec::new();
        for (name, pitch, quarters) in [("held", 60, 5), ("incoming", 72, 1)] {
            let Created::Scene(scene) = editor
                .execute(Command::AddScene { name: name.into() })
                .unwrap()
            else {
                panic!("scene")
            };
            let length = BeatTime::from_quarters(quarters).unwrap();
            let Created::SessionClip(clip) = editor
                .execute(Command::CreateSessionMidiClip {
                    track,
                    scene,
                    name: name.into(),
                    source_length: length,
                })
                .unwrap()
            else {
                panic!("clip")
            };
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: length,
                    pitch,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
            clips.push(clip);
        }
        let source = crate::OwnedAudioSource::try_resident(
            8000,
            vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0; 2]],
        )
        .ok()
        .unwrap();
        let mut cell = prepare(editor.project(), vec![(asset, source)]);
        let owner = cell.owner_on_control().unwrap();
        owner
            .test_apply_action(crate::PlaybackAction::LaunchAndPlay(
                crate::SessionPlaybackCommand::LaunchClip {
                    clip: clips[0],
                    quantization: crate::LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
        owner.test_render(&mut [[0.0; 2]; 1]).unwrap();
        owner
            .test_apply_action(crate::PlaybackAction::Session(
                crate::SessionPlaybackCommand::LaunchClip {
                    clip: clips[1],
                    quantization: crate::LaunchQuantization::FourQuarter,
                },
            ))
            .unwrap();
        let route = owner
            .inner
            .runtime
            .as_ref()
            .unwrap()
            .playback
            .session_route_status(0)
            .unwrap();
        assert_eq!(route.pending.unwrap().performance_frame, 32000);
        let mut remaining = 31996;
        while remaining != 0 {
            let count = remaining.min(64);
            owner.test_render(&mut [[0.0; 2]; 64][..count]).unwrap();
            remaining -= count;
        }
        assert_eq!(
            owner
                .inner
                .runtime
                .as_ref()
                .unwrap()
                .playback
                .test_current_sampler_route_proofs(0),
            Some((1, 1, 0))
        );
        let mut pcm = Vec::new();
        for count in partitions {
            let mut block = vec![[0.0; 2]; *count];
            owner.test_render(&mut block).unwrap();
            pcm.extend(block);
            if owner.test_transport().unwrap().1 == 32001 {
                assert_eq!(
                    owner
                        .inner
                        .runtime
                        .as_ref()
                        .unwrap()
                        .playback
                        .test_current_sampler_route_proofs(0),
                    Some((2, 1, 1))
                );
            }
        }
        // A's loop is audible and held through frame31999. At32000 A receives
        // NoteOff while B starts, then A's prepared release ends. B keeps looping.
        let release = release_frames(2.0 / 16000.0);
        let source = [[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]];
        let expected: Vec<_> = (31997_u64..32005)
            .map(|frame| {
                let a = HALF_SPEED[(frame % 8) as usize];
                let level = frame
                    .checked_sub(32000)
                    .map_or(1.0, |steps| release_level(steps, release));
                let b = frame
                    .checked_sub(32000)
                    .map_or([0.0; 2], |age| source[(age % 4) as usize]);
                [a[0] * level + b[0], a[1] * level + b[1]]
            })
            .collect();
        assert_pcm_near(&pcm, &expected);
        // Drain any remaining release before checking A's retired proof.
        let drain = (32000 + release).saturating_sub(32005) as usize;
        owner.test_render(&mut vec![[0.0; 2]; drain]).unwrap();
        let playback = &owner.inner.runtime.as_ref().unwrap().playback;
        let route = playback.session_route_status(0).unwrap();
        assert_eq!(
            route.owner,
            crate::SessionPlaybackOwner::Session { clip: clips[1] }
        );
        assert!(route.pending.is_none());
        assert_eq!(
            playback.test_current_sampler_route_proofs(0),
            Some((2, 1, 0))
        );
        results.push(pcm);
        cell.cleanup_on_control().unwrap();
    }
    assert!(results.windows(2).all(|r| r[0] == r[1]));
}

#[path = "console_tests/cue.rs"]
mod cue;

#[path = "console_tests/source_metadata.rs"]
mod source_metadata;

#[path = "console_tests/steal_density.rs"]
mod steal_density;

#[path = "console_tests/volume.rs"]
mod volume;
