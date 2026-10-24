//! Actual ordinary resource publication must retain its native source, just as
//! constructor publication does. Compiler facts alone are not lifetime custody.
use super::*;
use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
use std::collections::BTreeMap;

fn limits() -> hex_engine::AudioStorageLedgerReservation {
    hex_engine::AudioStorageLedgerReservation {
        data_rows: 4096,
        handle_rows: 4096,
        root_rows: 128,
        max_retained_bytes: 32 * 1024 * 1024,
        max_construction_peak_bytes: 32 * 1024 * 1024,
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_native_graph_retains_endpoint_until_its_original_graph_return() {
    let _serial = serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .configure_physical_callback_request(&mut callback, 8)
        .unwrap();
    let (mut data, errors, terminals) =
        crate::native_callback_owner::reserve_native(callback, Arc::clone(&control.status));
    let retirement = NativeRetirement::new(terminals, control.native_candidate_root());
    control.enable_native_custody().unwrap();
    control
        .configure_device_resource_publication_on_control(limits())
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 192 * 1024 * 1024,
                max_construction_peak_data_bytes: 192 * 1024 * 1024,
            },
            3,
            4096,
        )
        .unwrap();
    control
        .configure_native_source_preparation(2, source_reservation())
        .unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let identity =
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, editor.project().next_identity())
            .unwrap();
    let binding = source(&mut control, identity);
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: control.native_source_prepared_state(binding).unwrap(),
        })
        .unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "ordinary-original.wav".into(),
            sample_rate: 48_000,
            frames: 32,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 32,
            source_offset: 0,
            source_length: 32,
            looping: false,
        })
        .unwrap();
    let media = BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48_000, vec![[0.25, -0.5]; 32]).unwrap(),
    )]);
    let resources = PreparedDeviceResources::prepare_on_control(
        &ResidentDeviceAssets::default(),
        editor.project(),
        1_000_000,
        &AtomicBool::new(false),
    )
    .unwrap();
    control.publish_native_source_endpoint(binding).unwrap();
    allocation_free(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    let playback = loop {
        let result = control.with_native_effect_definition(&[binding], |definition| {
            let builtins = hex_devices::native_registry().unwrap();
            let mut definitions = builtins.definitions().to_vec();
            definitions.push(definition);
            let effects = NativeDeviceRegistry::new(&definitions).unwrap();
            let revision =
                hex_engine::PreparedRevision::prepare_with_device_resources_and_effect_registry(
                    editor.project(),
                    &media,
                    Arc::new(hex_engine::SineTable::new()),
                    None,
                    &hex_engine::audio_transform::AudioTransformPreparation::default(),
                    &resources,
                    1_000_000,
                    &AtomicBool::new(false),
                    &effects,
                )
                .unwrap();
            Ok(
                Playback::try_from_prepared_with_device_resources(Arc::new(revision), &resources)
                    .unwrap(),
            )
        });
        match result {
            Ok(playback) => break playback,
            Err(reason)
                if matches!(
                    reason.as_str(),
                    "native effect preparation is busy" | "native effect selection is busy"
                ) =>
            {
                assert!(Instant::now() < deadline, "original source compiler borrow");
                std::thread::yield_now();
            }
            Err(reason) => panic!("original source compiler: {reason}"),
        }
    };
    let context = hex_engine::AudioSourceContext {
        project_generation: 41,
        editor_revision: 1,
    };
    control
        .publish_with_admission_and_source_on_control(
            playback,
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("actual ordinary native publication"));
    control.command(PlaybackCommand::Play).unwrap();
    let root = control.native_candidate_root();
    for block in 0..3 {
        let mut output = [[0.0; 2]; 8];
        let mut host = [0.0_f32; 16];
        allocation_free(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.begin_native_effect_callback(8).unwrap();
                let rendered = callback.render(&mut output);
                assert!(rendered.is_ok());
                assert!(crate::device::finish_host_chunk(
                    callback,
                    &mut host,
                    &mut output,
                    2,
                    &rendered,
                    0,
                    Instant::now(),
                ));
            })
        });
        replacement::wait_for_blocks(&root, binding, block + 1);
        assert_eq!(
            output,
            if block < 2 {
                [[0.0; 2]; 8]
            } else {
                [[0.125, -0.25]; 8]
            }
        );
    }
    assert!(control.retire_native_source_preparation(binding));
    let mut kept_endpoint = false;
    allocation_free(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            assert!(matches!(
                callback.active,
                Some(ActivePublication::Legacy(_))
            ));
            kept_endpoint = callback.has_native_source_endpoints();
        })
    });
    // Always retire the actual original owners before asserting the observed
    // bug, so a pre-fix failure cannot strand the singleton lifecycle executor.
    let mut retirement = close_backend(data, errors, retirement);
    let returned = recover(&mut control, binding);
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    drop(control);
    finish(&mut retirement);
    assert!(
        kept_endpoint,
        "source retirement returned the endpoint while its ordinary graph was still current"
    );
}
