//! Actual service request resolution and prepare-before-commit publication.
use super::*;
use hex_engine::lfo_modulator::Settings;
use hex_lattice_contract::DeviceTypeId;
use hex_project::{DeviceModulation, ModulationSourceId, Modulator};

#[test]
fn modulation_edit_is_revision_bound_and_commits_only_after_resident_admission() {
    let observed = Arc::new(Observed::default());
    let runtime = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        1024,
        None,
        Box::new(OfflineSink {
            observed: Arc::clone(&observed),
            pending: None,
        }),
        crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: 2 * runtime,
                max_construction_peak_bytes: 3 * runtime,
            },
            max_scan_frames: 1024,
        },
    )
    .unwrap();
    let receive = || {
        service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap()
    };
    receive();
    let submit = |request| {
        service.submit(request).unwrap();
        receive()
    };
    let added = submit(Request::Edit(Command::InsertMainDevice {
        before: None,
        state: hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility"))
            .unwrap(),
    }));
    let Some(Created::Device(device)) = added.created else {
        panic!("main effect: {:?}", added.error);
    };
    let before = added.state.revision;
    let edit = |revision, modulation| Request::ReplaceDeviceModulation {
        project_generation: 0,
        revision,
        device,
        modulation,
    };
    let bank = DeviceModulation::new(
        vec![Modulator {
            color: Default::default(),
            id: ModulationSourceId::new(added.state.project.next_identity()).unwrap(),
            enabled: true,
            state: Settings::default().encode().unwrap(),
        }],
        vec![],
    )
    .unwrap();
    let accepted = submit(edit(before, bank.clone()));
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert_eq!(accepted.state.revision, before + 1);
    assert_eq!(
        accepted.state.project.device(device).unwrap().modulation(),
        &bank
    );
    let publication_count = observed.publications.lock().unwrap().len();
    assert_eq!(
        observed
            .publications
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .0
            .context
            .editor_revision,
        accepted.state.revision,
    );

    let stale = submit(edit(before, DeviceModulation::default()));
    assert_eq!(
        stale.error.as_deref(),
        Some("modulation edit cancelled · project changed")
    );
    assert_eq!(stale.state.revision, accepted.state.revision);
    assert_eq!(
        observed.publications.lock().unwrap().len(),
        publication_count
    );

    observed.refuse.store(true, Ordering::Release);
    let refused = submit(edit(accepted.state.revision, DeviceModulation::default()));
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|error| error.contains("fixture publication refused"))
    );
    assert_eq!(refused.state.revision, accepted.state.revision);
    assert_eq!(
        refused.state.project.device(device).unwrap().modulation(),
        &bank
    );
    assert_eq!(
        observed.publications.lock().unwrap().len(),
        publication_count
    );
}
