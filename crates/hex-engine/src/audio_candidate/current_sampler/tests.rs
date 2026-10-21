use super::*;
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::sync::atomic::AtomicBool;
fn fixture() -> (Project, hex_project::AssetId) {
    let mut editor = Editor::new(Project::new(16000).unwrap(), 8).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "original.wav".into(),
            sample_rate: 8000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
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
    let mut state = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
    state.mode = hex_devices::sampler::state::AuthoredMode::OneShot {
        gate: hex_devices::sampler::GateMode::Trigger,
    };
    state.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.0,
    };
    let native = state
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    editor
        .execute(Command::InsertInstrument {
            track,
            state: native,
        })
        .unwrap();
    (editor.project().clone(), asset)
}
#[test]
fn current_sampler_actual_constructor_has_literal_pcm_stop_replay_and_closed_custody() {
    assert!(crate::audio_storage_constructor_layout_supported());
    let (project, asset) = fixture();
    assert!(crate::native_device_resources::current_sampler::validate_project(&project).unwrap());
    let quote = AudioCandidateReservationQuote::for_project(&project, 1).unwrap();
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
        &project,
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
            "{e:?}; custody={:?}/{:?}, token={}, runtime={}",
            cell.custody.as_ref().map(|c| c.phase()),
            cell.custody.as_ref().and_then(|c| c.reason()),
            cell.token.is_some(),
            cell.draft.as_ref().is_some_and(|d| d.runtime.is_some())
        )
    });
    let owner = cell.owner_on_control().unwrap();
    owner
        .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
            data_rows: quote.data_rows,
            handle_rows: quote.handle_rows,
            root_rows: 8,
            max_retained_bytes: limits.max_prepared_data_bytes,
            max_construction_peak_bytes: limits.max_construction_data_bytes,
        })
        .unwrap();
    let runtime = owner.inner.runtime.as_ref().unwrap();
    assert_ne!(
        runtime
            .sampler_sources
            .as_ref()
            .unwrap()
            .0
            .bank
            .source_view(
                &runtime
                    .sampler_sources
                    .as_ref()
                    .unwrap()
                    .0
                    .bank
                    .source_token(asset)
                    .unwrap()
            )
            .unwrap()
            .resident_frames()
            .unwrap()
            .as_ptr(),
        original
            .source_view(&token)
            .unwrap()
            .resident_frames()
            .unwrap()
            .as_ptr()
    );
    owner
        .test_apply_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::Play,
        ))
        .unwrap();
    let mut output = [[0.0; 2]; 12];
    owner.test_render(&mut output).unwrap();
    assert_eq!(
        output,
        [
            [0.125, -0.25],
            [0.3125, -0.125],
            [0.5, 0.0],
            [0.1875, 0.125],
            [-0.125, 0.25],
            [-0.0625, 0.125],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0]
        ]
    );
    let before = owner.test_transport();
    assert!(
        owner
            .test_apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Seek(1)
            ))
            .is_err()
    );
    assert_eq!(before, owner.test_transport());
    owner
        .test_apply_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::Stop,
        ))
        .unwrap();
    owner
        .test_apply_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::Play,
        ))
        .unwrap();
    let mut replay = [[0.0; 2]; 12];
    owner.test_render(&mut replay).unwrap();
    assert_eq!(replay, output);
    cell.cleanup_on_control().unwrap();
    assert!(cell.owner_on_control().is_none());
    assert!(original.source_view(&token).is_ok());
}

#[test]
fn sampler_compiler_reserves_empty_clip_and_note_workspace_before_entering() {
    for note_count in [0, 21, 256] {
        let (project, asset) = fixture();
        let mut editor = Editor::new(project, 8).unwrap();
        let track = editor.project().tracks()[0].id();
        let clip = editor.project().tracks()[0].midi_clips()[0].id();
        editor.execute(Command::RemoveMidiClip(clip)).unwrap();
        if note_count != 0 {
            let Created::Clip(clip) = editor
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::from_raw(BeatTime::ONE.raw() * note_count),
                })
                .unwrap()
            else {
                panic!("clip")
            };
            for i in 0..note_count {
                editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::from_raw(BeatTime::ONE.raw() * i),
                        duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
                        pitch: 60,
                        channel: 0,
                        velocity: 127,
                    })
                    .unwrap();
            }
        } else {
            for i in 0..128 {
                editor
                    .execute(Command::PlaceMidiClip {
                        track,
                        start: BeatTime::from_raw(BeatTime::ONE.raw() * i),
                        length: BeatTime::ONE,
                    })
                    .unwrap();
            }
        }
        let project = editor.project();
        let quote = AudioCandidateReservationQuote::for_project(project, 1).unwrap();
        let original_limits = quote.storage_reservation(65536).unwrap();
        let mut peak = 0;
        let mut retained = 0;
        for pass in 0..4 {
            let mut limits = original_limits;
            if pass > 0 {
                limits.max_construction_data_bytes = peak - usize::from(pass == 1);
                limits.max_prepared_data_bytes = retained - usize::from(pass == 3);
            }
            let source = crate::OwnedAudioSource::try_resident(8000, vec![[0.0; 2]; 4])
                .ok()
                .unwrap();
            let mut bank = Some(
                crate::OwnedAudioInputs::try_new(vec![(asset, source)], limits)
                    .ok()
                    .unwrap(),
            );
            let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
            crate::midi_compile::CURRENT_SAMPLER_COMPILER_ENTRIES.with(|n| n.set(0));
            let result = custody.begin_current_sampler_in_place_on_control(
                project,
                11,
                crate::AudioSourceContext {
                    project_generation: 4,
                    editor_revision: 7,
                },
                &mut bank,
                4,
                &AtomicBool::new(false),
            );
            let entered = crate::midi_compile::CURRENT_SAMPLER_COMPILER_ENTRIES.with(|n| n.get());
            if pass == 1 || pass == 3 {
                assert!(
                    matches!(result, Err(ref e) if e.reason == AudioStorageError::ReservationExceeded)
                );
                assert_eq!(
                    entered, 0,
                    "under-budget constructor must refuse BEFORE compiler"
                );
                assert!(bank.is_some(), "refusal preserves actual original bank");
            } else {
                assert!(
                    result.is_ok(),
                    "exact compiler peak failed: {:?}",
                    result.err().map(|e| e.reason)
                );
                assert_eq!(entered, 1);
                let report = custody.capacity_report();
                assert!(report.native_compiler_workspace_bytes > 0);
                assert!(
                    report.construction_peak_bytes
                        >= report.retained_stage_bytes + report.native_compiler_workspace_bytes
                );
                if pass == 0 {
                    peak = report.construction_peak_bytes;
                    retained = report.retained_stage_bytes;
                }
            }
            custody.cleanup_on_control();
        }
    }
}
