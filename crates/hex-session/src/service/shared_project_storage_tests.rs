use super::*;
use hex_engine::{AudioRootStorageLedger, AudioStorageLedgerReservation};
fn reservation() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 128,
        root_rows: 16,
        max_retained_bytes: 2_000_000,
        max_construction_peak_bytes: 2_000_000,
    }
}
#[test]
fn actual_canonical_views_snapshots_and_two_preview_producers_retain_paired_project_fields() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    eprintln!("executing actual canonical ViewState/snapshot/tuning/device preview project fields");
    let mut session = Session::new(48_000, 8, 1024).unwrap();
    let Created::Track(track) = session
        .command(Command::AddTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = session
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let first = view_state(
        &session,
        hex_engine::empty_midi_preview_on_control(),
        1,
        1,
        false,
        None,
    );
    let second = view_state(
        &session,
        hex_engine::empty_midi_preview_on_control(),
        1,
        1,
        false,
        None,
    );
    assert_eq!(first.project, second.project);
    assert!(!first.project.shares_owner_with(&second.project));
    let source = first
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .project_owner_on_control();
    assert!(first.project.shares_owner_with(source));
    let (tuning_project, tuning_prepared) = session
        .preview_tuning(
            hex_tuning::ProjectTuning::default().with_enabled(true),
            None,
        )
        .unwrap();
    let tuning = preview_view_state(&first, tuning_project, tuning_prepared);
    let (device_project, device_prepared) = session
        .preview_device_parameter(
            device,
            hex_engine::DEVICE_OUTPUT_GAIN.id,
            hex_lattice_contract::ParameterValue::Float(-6.0),
            None,
        )
        .unwrap();
    let parameter = preview_view_state(&first, device_project, device_prepared);
    assert!(!first.project.shares_owner_with(&tuning.project));
    assert!(!first.project.shares_owner_with(&parameter.project));
    let collect = |reservation| {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
        ledger.borrow_shared_metadata_body_on_control(&first.project)?;
        let body = ledger.summary()?.data_bytes;
        assert!(body > std::mem::size_of::<Project>());
        assert!(
            ledger
                .validate_selected_shared_handles_on_control()
                .is_err()
        ); // actual snapshot field omitted
        ledger.borrow_shared_metadata_body_on_control(source)?;
        ledger.validate_selected_shared_handles_on_control()?;
        for view in [&second, &tuning, &parameter] {
            ledger.borrow_shared_metadata_body_on_control(&view.project)?;
        }
        assert!(
            ledger
                .validate_selected_shared_handles_on_control()
                .is_err()
        ); // second snapshot field omitted
        ledger.borrow_shared_metadata_body_on_control(
            second
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .project_owner_on_control(),
        )?;
        ledger.validate_selected_shared_handles_on_control()?;
        assert_eq!(ledger.summary()?.data_bytes, body * 4);
        assert_eq!(ledger.selected_shared_counts_on_control(), (4, 6));
        assert!(ledger.summary()?.require_complete().is_err()); // selected outer owners only
        ledger.summary()
    };
    let summary = collect(reservation()).unwrap();
    let mut exact = reservation();
    exact.max_retained_bytes = summary.retained_bytes;
    exact.max_construction_peak_bytes = summary.construction_peak_bytes;
    assert_eq!(collect(exact).unwrap(), summary);
    exact.max_retained_bytes -= 1;
    assert!(collect(exact).is_err());
    drop(session);
    drop(first);
    assert_eq!(second.project.track(track).unwrap().name(), "source");
    assert_eq!(
        parameter
            .project
            .device(device)
            .unwrap()
            .state()
            .output_gain_millidb(),
        -6000
    );
}
