//! Recoverable renderer failures must close every speculative bus transaction.
use super::*;
#[test]
fn main_render_error_closes_transaction_for_immediate_handoff_and_retry() {
    use hex_lattice::conformance::owned_delay;
    use hex_project::{Command, Created, Editor};
    let mut edit = Editor::new(hex_project::Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(source) = edit
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("source")
    };
    let Created::Track(target) = edit
        .execute(Command::AddTrack {
            name: "target".into(),
        })
        .unwrap()
    else {
        panic!("target")
    };
    let Created::Asset(asset) = edit
        .execute(Command::AddAsset {
            path: "memory/rollback.wav".into(),
            sample_rate: 48_000,
            frames: 128,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    edit.execute(Command::PlaceClip {
        track: source,
        asset,
        start: 0,
        duration: 128,
        source_offset: 0,
        source_length: 128,
        looping: false,
    })
    .unwrap();
    edit.execute(Command::SetAudioFeed {
        source: hex_project::RoutingOwner::Track(source),
        destination: target,
        gain: 0.5,
    })
    .unwrap();
    edit.execute(Command::InsertMainDevice {
        before: None,
        state: owned_delay::state(17).unwrap(),
    })
    .unwrap();
    let definitions: [&dyn hex_lattice::NativeDeviceDefinition; 1] = [&owned_delay::DEFINITION];
    let registry = hex_lattice::NativeDeviceRegistry::new(&definitions).unwrap();
    let media = std::collections::BTreeMap::from([(
        asset,
        Audio::new(
            48_000,
            (0..128)
                .map(|n| [0.125 + n as f32 * 0.001, -0.25])
                .collect(),
        )
        .unwrap(),
    )]);
    let project = compile::project_with_registry(edit.project(), &media, &registry).unwrap();
    let mut failed = RendererRuntime::new(&project);
    let mut warm_position = 0;
    failed
        .render_without_pages(&project, &mut warm_position, true, &mut [[0.0; 2]; 32])
        .unwrap();
    let mut position = u64::MAX - 8;
    let original_position = position;
    let mut output = [[0.25; 2]; 4];
    // Actual compensated frame-to-beat conversion overflows after begin_block,
    // without injecting a fake error or mutating the prepared graph.
    assert!(
        failed
            .render_without_pages(&project, &mut position, true, &mut output)
            .is_err()
    );
    assert_eq!(position, original_position);
    assert_eq!(output, [[0.0; 2]; 4]);
    let mut next = RendererRuntime::new(&project);
    assert!(
        next.can_inherit_from(&project, &failed, &project),
        "failed Main transaction must be closed before any reset/retry"
    );
    next.inherit_from(&project, &failed, &project);
    let mut retry_position = warm_position;
    next.render_without_pages(&project, &mut retry_position, true, &mut output)
        .unwrap();
    let mut oracle = Renderer::new(&project);
    oracle.play();
    oracle.render(&mut [[0.0; 2]; 32]).unwrap();
    let mut expected = [[0.0; 2]; 4];
    oracle.render(&mut expected).unwrap();
    assert!(expected.iter().flatten().any(|sample| *sample != 0.0));
    assert_eq!(output, expected);
    assert_eq!(retry_position, oracle.position());
}

#[test]
fn global_main_audition_cancellation_preserves_normal_pcm_after_restart() {
    use hex_lattice::{ProcessorAuditionSelection, ProcessorChainOwner};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
    use hex_project::{Command, Created, Editor};
    let mut edit = Editor::new(hex_project::Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { asset, .. } = edit
        .execute(Command::ImportPreparedAudioTrack {
            name: "signal".into(),
            path: "memory/audition.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 2048,
            playback_frames: 2048,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let mut state = hex_devices::eq8::EQ8_DEFINITION.default_state().unwrap();
    for parameter in [1, 41] {
        state = crate::patch_native_device_parameter(
            &state,
            ParameterId::from_static(parameter),
            ParameterValue::Bool(true),
        )
        .unwrap();
    }
    state = crate::patch_native_device_parameter(
        &state,
        hex_devices::eq8::parameter::enabled(0, 1),
        ParameterValue::Bool(true),
    )
    .unwrap();
    state = crate::patch_native_device_parameter(
        &state,
        hex_devices::eq8::parameter::gain(0, 1),
        ParameterValue::Float(12.0),
    )
    .unwrap();
    let Created::Device(device) = edit
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("device")
    };
    let media = std::collections::BTreeMap::from([(
        asset,
        Audio::new(
            48_000,
            (0..2048)
                .map(|n| {
                    let sample = (n as f32 * 0.33).sin() * 0.3 + (n as f32 * 0.01).sin() * 0.2;
                    [sample, -sample * 0.5]
                })
                .collect(),
        )
        .unwrap(),
    )]);
    let revision = Arc::new(
        PreparedRevision::prepare(edit.project(), &media, crate::SharedSineTable::new_on_control(), None)
            .unwrap(),
    );
    let identity = PostNodeIdentity::new(
        ProcessorChainOwner::Main,
        device.value(),
        DeviceTypeId::from_static("hex.eq8"),
    );
    let selection = ProcessorAuditionSelection { index: 0, lane: 0 };
    let mut audition = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut normal = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    for runtime in [&mut audition, &mut normal] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    assert!(audition.select_processor_audition(identity, Some(selection)));
    let mut actual = [[0.0; 2]; 256];
    let mut expected = actual;
    audition.render(&mut actual).unwrap();
    normal.render(&mut expected).unwrap();
    assert!(
        actual.iter().zip(&expected).any(|(a, b)| a != b),
        "real audition must alter PCM"
    );
    assert!(audition.processor_audition_is_active(identity, selection));
    // This is the production hard-boundary operation shared by Stop/shutdown
    // and stale-target rejection, rather than a private chain reset.
    audition.cancel_all_processor_auditions();
    assert!(!audition.processor_audition_is_active(identity, selection));
    for runtime in [&mut audition, &mut normal] {
        runtime.apply(SessionPlaybackCommand::Stop).unwrap();
        runtime.apply(SessionPlaybackCommand::Seek(0)).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    audition.render(&mut actual).unwrap();
    normal.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}
