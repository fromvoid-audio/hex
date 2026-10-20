use super::*;
use crate::mutable_instrument::{MutableInstrumentTail, MutableInstrumentTrackRuntime};
use crate::native_device_resources::{DeviceResourceConfiguration, DeviceResourcePreparation};
use crate::{AudioStorageLedgerReservation, AudioStorageRole, PreparedRevision};
use std::{collections::BTreeMap, mem::size_of, sync::Arc};

fn reservation() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 128,
        root_rows: 32,
        max_retained_bytes: 8 * 1024 * 1024,
        max_construction_peak_bytes: 8 * 1024 * 1024,
    }
}
#[inline(never)]
fn runtime(revision: Arc<PreparedRevision>) -> Box<SessionPlayback> {
    Box::new(SessionPlayback::new(revision).unwrap())
}
#[test]
fn parented_sampler_runtime_storage_charges_actual_inline_configuration_once() {
    let Some((source, identity, editor)) =
        crate::shared_analytic::sampler_tests::canonical_fixture()
    else {
        return;
    };
    drop(identity);
    let revision = Arc::new(
        PreparedRevision::prepare_with_audio_transforms_and_device_resources(
            editor.project(),
            &BTreeMap::<hex_project::AssetId, crate::Audio>::new(),
            crate::SharedSineTable::new_on_control(),
            None,
            &crate::audio_transform::AudioTransformPreparation::default(),
            DeviceResourcePreparation {
                bank: &source,
                max_scope_metadata_bytes: 65_536,
            },
        )
        .unwrap(),
    );
    let session = runtime(revision);
    let count = session.mutable_instruments.iter().flatten().count();
    assert_eq!(count, 1);
    assert!(session.tail_mutable_instruments.iter().all(Option::is_none));
    let mut resource_only = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    resource_only
        .borrow_session_mutable_resource_owners_on_control(&session, AudioStorageRole::Current)
        .unwrap();
    let selected = resource_only.summary().unwrap();
    drop(resource_only);
    let mut full = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    if env!("HEX_SAMPLER_BOX_LAYOUT_KNOWN") != "1" {
        assert!(
            full.borrow_session_mutable_runtime_storage_on_control(
                &session,
                AudioStorageRole::Current
            )
            .is_err()
        );
        return;
    }
    full.borrow_session_mutable_runtime_storage_on_control(&session, AudioStorageRole::Current)
        .unwrap();
    eprintln!(
        "SUPPORTED actual parented Sampler runtime Box/Vec/configuration independent byte accounting"
    );
    let summary = full.summary().unwrap();
    let pair_bytes =
        2 * hex_devices::sampler::PreparedSamplerRuntime::runtime_box_layout_on_control().size();
    let occurrence_bytes = session
        .mutable_instruments
        .iter()
        .flatten()
        .map(|runtime| runtime.ledger_capacity_bytes().unwrap())
        .sum::<usize>();
    let parent_bytes = size_of::<SessionPlayback>()
        + session.mutable_instruments.capacity()
            * size_of::<Option<MutableInstrumentTrackRuntime>>()
        + session.tail_mutable_instruments.capacity() * size_of::<Option<MutableInstrumentTail>>();
    // The resource-only view charged its standalone field. In the complete
    // runtime view that SAME field is already inside the actual containing Vec.
    let expected_delta = parent_bytes + pair_bytes + occurrence_bytes
        - count * size_of::<DeviceResourceConfiguration>();
    assert_eq!(summary.data_bytes - selected.data_bytes, expected_delta);
    assert_eq!(summary.data_allocations - selected.data_allocations, 6); // 2Boxes+2occurrenceVecs+2parentVecs
    assert!(summary.require_complete().is_err()); // other actual Session/revision/source roots remain unvisited
    drop(full);
    let mut exact = reservation();
    exact.max_retained_bytes = summary.retained_bytes;
    exact.max_construction_peak_bytes = summary.construction_peak_bytes;
    let mut full = AudioRootStorageLedger::try_new_on_control(exact).unwrap();
    full.borrow_session_mutable_runtime_storage_on_control(&session, AudioStorageRole::Current)
        .unwrap();
    assert_eq!(full.summary().unwrap().data_bytes, summary.data_bytes);
    drop(full);
    exact.max_retained_bytes -= 1;
    let mut short = AudioRootStorageLedger::try_new_on_control(exact).unwrap();
    let result = short
        .borrow_session_mutable_runtime_storage_on_control(&session, AudioStorageRole::Current);
    assert!(result.is_err() || short.summary().is_err());
}
