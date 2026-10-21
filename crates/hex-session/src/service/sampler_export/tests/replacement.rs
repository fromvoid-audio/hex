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
