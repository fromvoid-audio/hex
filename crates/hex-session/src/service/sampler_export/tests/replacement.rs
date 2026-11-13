//! Same-Session decode allowance includes unused originals and captured jobs.
use super::*;
#[test]
fn appended_original_allowance_counts_unassigned_old_bank_native_hold_and_captured_export_before_ingress()
 {
    let files = Files::new(true);
    let (mut session, current) = direct_sampler(&files);
    let extra = Files::new(false);
    let decoded = decode_wav_file(
        &extra.source,
        Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT),
    )
    .unwrap();
    let (_, current2) = session
        .import_service_snapshot(
            decoded,
            &AtomicBool::new(false),
            Admission {
                current: Some(current.selection()),
                ..Default::default()
            },
        )
        .unwrap();
    drop(current);
    assert_eq!(session.project().assets().count(), 2);
    session.max_resident_frames = 20;
    let external = Files::new(false);
    let (external_session, export) = direct_sampler(&external);
    drop(external_session);
    let source = export.sources.as_ref().unwrap();
    let (_job, obligation) = source.capture_for_dispatch_on_control(1).unwrap();
    let no_export = Admission {
        current: Some(current2.selection()),
        native: NativeSamplerReservationHold {
            frames: 4,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        no_export.append_original_frame_allowance(&session).unwrap(),
        8
    );
    let with_export = Admission {
        export: Some(export.selection()),
        outbound: Some(resources::ControlSamplerOutbound {
            anchor: source,
            obligation: &obligation,
            job: 1,
        }),
        runtime_bytes: hex_engine::SessionPlayback::control_sampler_export_runtime_allowance(
            &export.prepared,
            source,
        )
        .unwrap()
            + std::mem::size_of_val(&obligation),
        ..no_export
    };
    assert_eq!(
        with_export
            .append_original_frame_allowance(&session)
            .unwrap(),
        4
    );
    let incoming = Files::new(true);
    let mut bytes = fs::read(&incoming.source).unwrap();
    bytes[4..8].copy_from_slice(&60_u32.to_le_bytes());
    bytes[40..44].copy_from_slice(&24_u32.to_le_bytes());
    for _ in 0..4 {
        bytes.extend_from_slice(&8192_i16.to_le_bytes());
    }
    fs::write(&incoming.source, bytes).unwrap();
    let decoded = decode_wav_file(
        &incoming.source,
        Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT),
    )
    .unwrap();
    assert_eq!(decoded.frames(), 6);
    let before = session.project().clone();
    let storage = session.native_storage_on_control().unwrap();
    let overview = session.source_overviews();
    assert!(
        no_export
            .inspect_appended_original(&session, Some(decoded.audio()))
            .is_ok()
    );
    assert!(
        with_export
            .inspect_appended_original(&session, Some(decoded.audio()))
            .is_err()
    );
    assert_eq!(session.project(), &before);
    assert_eq!(session.native_storage_on_control().unwrap(), storage);
    assert!(session.source_overviews().shares_storage_with(&overview));
}

#[test]
fn replacement_history_requires_exact_forward_template_and_shared_catalog_in_both_directions() {
    use super::super::super::sampler_parameter::validate_history_pair;
    let files = Files::new(true);
    let (session, _source) = direct_sampler(&files);
    let mut editor = hex_project::Editor::new(session.project().clone(), 16).unwrap();
    let track = editor.project().tracks()[0].id();
    let device = editor.project().track(track).unwrap().instrument().unwrap();
    let id = device.id();
    let old = device.state().clone();
    let (state, source_slot) =
        hex_engine::sampler_replacement_template_on_control(&old, 6).unwrap();
    editor
        .execute(Command::ImportDeviceAssetReplacingInstrument {
            owner: hex_project::InstrumentAssetOwner::Track(track),
            device: id,
            expected_state: old,
            path: files.output.clone(),
            sample_rate: 12000,
            frames: 6,
            state,
            source_slot,
        })
        .unwrap();
    editor.inspect_history(false, |current, target| {
        assert!(validate_history_pair(current, target).is_ok());
        for (parameter, value) in [
            (hex_engine::SamplerParameter::Gain, 0.5),
            (hex_engine::SamplerParameter::Transpose, 12.0),
            (hex_engine::SamplerParameter::StartFrame, 1.0),
            (hex_engine::SamplerParameter::LoopEnabled, 1.0),
        ] {
            let mut altered = hex_project::Editor::new(current.clone(), 1).unwrap();
            let state = hex_engine::patch_sampler_parameter_on_control(
                parameter,
                current.device(id).unwrap().state(),
                value,
            )
            .unwrap();
            altered
                .execute(Command::ReplaceDeviceState { device: id, state })
                .unwrap();
            assert!(
                validate_history_pair(altered.project(), target).is_err(),
                "{parameter:?}"
            );
        }
        let mut unrelated = hex_project::Editor::new(current.clone(), 1).unwrap();
        unrelated
            .execute(Command::ImportDeviceAsset {
                path: files.source.clone(),
                sample_rate: 8000,
                frames: 4,
            })
            .unwrap();
        assert!(validate_history_pair(unrelated.project(), target).is_err());
    });
    editor
        .undo_prepared_transition::<(), hex_project::Invalid>(|a, b| {
            assert!(validate_history_pair(a, b).is_ok());
            Ok(())
        })
        .unwrap()
        .unwrap();
    assert!(
        editor
            .inspect_history(true, validate_history_pair)
            .unwrap()
            .is_ok()
    );
    editor
        .redo_prepared_transition::<(), hex_project::Invalid>(|a, b| {
            assert!(validate_history_pair(a, b).is_ok());
            Ok(())
        })
        .unwrap()
        .unwrap();
}
