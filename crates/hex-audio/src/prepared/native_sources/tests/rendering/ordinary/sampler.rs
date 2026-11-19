//! Original registered sampler resources through actual ordinary/native return.
use super::*;
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
use hex_project::BeatTime;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_sampler_and_native_effect_keep_original_resources_across_epochs() {
    let _serial = serial();
    // Match the existing production preparation worker's bounded stack.
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(sampler_epochs)
        .unwrap()
        .join()
        .unwrap();
}

fn sampler_epochs() {
    let mut f = fixture("processing");
    let track = f.editor.project().tracks()[0].id();
    let Created::Asset(asset) = f
        .editor
        .execute(Command::ImportDeviceAsset {
            path: "original-device.wav".into(),
            sample_rate: 48_000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("device asset")
    };
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
    f.editor
        .execute(Command::InsertInstrument {
            track,
            state: state
                .to_native()
                .unwrap()
                .with_asset_references(vec![DeviceAssetReference {
                    slot: DeviceAssetSlot::new(1).unwrap(),
                    asset: DeviceAssetKey::new(asset.value()).unwrap(),
                }])
                .unwrap(),
        })
        .unwrap();
    let Created::Clip(clip) = f
        .editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("sampler midi clip")
    };
    f.editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let cancel = AtomicBool::new(false);
    let mut store = ResidentDeviceAssets::default();
    store
        .insert_on_control(
            (
                asset,
                hex_engine::OwnedAudioSource::try_resident(
                    48_000,
                    vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0; 2]],
                )
                .ok()
                .unwrap(),
            ),
            0,
            limits(),
            4,
            &cancel,
            || Ok::<_, hex_engine::native_device_resources::ResourcePreparationError>(()),
        )
        .unwrap();
    let original = store
        .source(asset)
        .unwrap()
        .resident_frames()
        .unwrap()
        .as_ptr();
    let facts = store.original_source_facts_on_control(asset).unwrap();
    f.resources =
        PreparedDeviceResources::prepare_on_control(&store, f.editor.project(), 1_000_000, &cancel)
            .unwrap();
    let canonical = f.editor.project().clone();
    let saved = f.control.native_source_prepared_state(f.binding).unwrap();
    // Independent dry graph uses the same retained source, with only the Main
    // foreign node removed. Its sampler genuinely contributes to this oracle.
    let mut dry_editor = Editor::new(canonical.clone(), 8).unwrap();
    dry_editor
        .execute(Command::RemoveDevice(canonical.main_devices()[0].id()))
        .unwrap();
    let dry_resources = PreparedDeviceResources::prepare_on_control(
        &store,
        dry_editor.project(),
        1_000_000,
        &cancel,
    )
    .unwrap();
    let dry_revision = Arc::new(
        hex_engine::PreparedRevision::prepare_with_device_resources(
            dry_editor.project(),
            &f.media,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &dry_resources,
            1_000_000,
            &cancel,
        )
        .unwrap(),
    );
    let mut dry =
        Playback::try_from_prepared_with_device_resources(dry_revision, &dry_resources).unwrap();
    dry.apply(PlaybackCommand::Play);
    let mut reference = [[0.0; 2]; 8];
    dry.render(&mut reference).unwrap();
    assert_ne!(reference[..4], [[0.25, -0.5]; 4]);
    let root = f.control.native_candidate_root();
    let playback = compile(&f, true);
    // A prepared revision alone cannot reconstruct the original sampler bank.
    assert!(
        Playback::try_from_prepared_with_session(Arc::clone(playback.prepared_revision().unwrap()))
            .is_err()
    );
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    for epoch_index in 0..2_u64 {
        f.control.command(PlaybackCommand::Play).unwrap();
        for n in 0..3_u64 {
            let output = block(&mut f);
            replacement::wait_for_blocks(&root, f.binding, epoch_index * 3 + n + 1);
            let gain = 0.5 + epoch_index as f32 * 3.0 * 0.125;
            let expected = if n < 2 {
                [[0.0; 2]; 8]
            } else {
                reference.map(|frame| [frame[0] * gain, frame[1] * gain])
            };
            assert_eq!(
                output, expected,
                "registered sampler/native epoch {epoch_index}"
            );
        }
        assert_eq!(f.editor.project(), &canonical);
        assert_eq!(
            store
                .source(asset)
                .unwrap()
                .resident_frames()
                .unwrap()
                .as_ptr(),
            original
        );
        assert_eq!(
            store.original_source_facts_on_control(asset).unwrap(),
            facts
        );
        assert_eq!(
            f.control.native_source_prepared_state(f.binding),
            Some(saved.clone())
        );
        if epoch_index == 1 {
            break;
        }
        let source = f.control.status.native_effect_source().unwrap();
        f.control.stop();
        let fence = f.control.request_native_epoch_fence(source).unwrap();
        allocation_free(|| {
            f.data
                .owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
        f.control
            .return_stopped_ordinary_native_epoch_runtime(source, fence)
            .unwrap();
        for _ in 0..2 {
            allocation_free(|| {
                f.data
                    .owner_mut()
                    .unwrap()
                    .run(|callback, _| callback.begin_block())
            });
        }
        renew_cohort(&mut f);
        // Reuse the exact prepared resource set after the actual old graph and
        // native endpoints return; no decode, import or resource substitution.
        let playback = compile(&f, true);
        let revision = publish(
            &mut f.control,
            playback,
            PublicationPolicy::InstallUnderNativeEpoch(fence),
        );
        assert_eq!(block(&mut f), [[0.0; 2]; 8]);
        assert_eq!(f.control.status.applied_revision.load(Acquire), revision);
        f.control.release_stopped_edit_fence(fence);
    }
    let returned = dispose(f);
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
        if retired.status.success() && retired.terminal.is_some_and(|terminal|
            (terminal.blocks, terminal.frames) == (6, 48)))
    );
    assert_eq!(
        store
            .source(asset)
            .unwrap()
            .resident_frames()
            .unwrap()
            .as_ptr(),
        original
    );
}
