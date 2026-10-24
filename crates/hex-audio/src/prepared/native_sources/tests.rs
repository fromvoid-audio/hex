//! Real source/worker and actual callback-carrier transfer; no Hex app/device.
use super::*;
use crate::candidate_custody::native_source_test_fixture::{identity, source_reservation};
use crate::native_callback_owner::{
    ErrorCallbackOwner, Guard, NativeCallbackOwner, NativeRetirement,
};
use crate::prepared::candidate_tests::{finish, serial};
use std::time::{Duration, Instant};

fn setup() -> (
    Control,
    Guard<NativeCallbackOwner>,
    Guard<ErrorCallbackOwner>,
    NativeRetirement,
    Binding,
) {
    setup_for_identity(identity())
}
fn setup_for_identity(
    source_identity: hex_lattice::ProcessorInstanceIdentity,
) -> (
    Control,
    Guard<NativeCallbackOwner>,
    Guard<ErrorCallbackOwner>,
    NativeRetirement,
    Binding,
) {
    setup_for_identity_with_fixture(source_identity, "processing")
}
fn setup_for_identity_with_fixture(
    source_identity: hex_lattice::ProcessorInstanceIdentity,
    fixture: &str,
) -> (
    Control,
    Guard<NativeCallbackOwner>,
    Guard<ErrorCallbackOwner>,
    NativeRetirement,
    Binding,
) {
    setup_for_identity_with_profile(source_identity, fixture, 8)
}
fn setup_for_identity_with_profile(
    source_identity: hex_lattice::ProcessorInstanceIdentity,
    fixture: &str,
    quantum: u16,
) -> (
    Control,
    Guard<NativeCallbackOwner>,
    Guard<ErrorCallbackOwner>,
    NativeRetirement,
    Binding,
) {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .configure_physical_callback_request(&mut callback, quantum)
        .unwrap();
    let (data, errors, terminals) =
        crate::native_callback_owner::reserve_native(callback, Arc::clone(&control.status));
    let retirement = NativeRetirement::new(terminals, control.native_candidate_root());
    control.enable_native_custody().unwrap();
    // Use the public constructor row quote for this fixture's one-source,
    // one-track topology (with a return only for the actual Return owner).
    // These are caller capacities; stage still validates the sealed summary.
    let mut topology =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
    topology
        .execute(hex_project::Command::AddTrack {
            name: "source".into(),
        })
        .unwrap();
    if matches!(
        source_identity.owner(),
        hex_lattice::ProcessorChainOwner::Return(_)
    ) {
        topology
            .execute(hex_project::Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap();
    }
    let rows =
        hex_engine::AudioCandidateReservationQuote::for_project(topology.project(), 1).unwrap();
    control
        .configure_audio_candidate_publication(crate::AudioCandidatePublicationReservation {
            runtime_slots: 3,
            archive_slots: 3,
            member_rows: rows.member_rows.checked_mul(3).unwrap(),
            transfer_rows: rows.transfer_rows.checked_mul(3).unwrap(),
            receipt_rows: 4,
            data_rows: 4096,
            handle_rows: 4096,
            // Two original native-source peak reservations coexist with the
            // full effect-aware revision quote during rooted preparation.
            max_retained_data_bytes: 192 * 1024 * 1024,
            max_construction_peak_data_bytes: 192 * 1024 * 1024,
        })
        .unwrap();
    control
        .configure_native_source_preparation(2, source_reservation())
        .unwrap();
    let binding = source_for_profile(&mut control, source_identity, fixture, quantum, None);
    (control, data, errors, retirement, binding)
}
fn source(control: &mut Control, identity: hex_lattice::ProcessorInstanceIdentity) -> Binding {
    source_for_fixture(control, identity, "processing")
}
fn source_for_fixture(
    control: &mut Control,
    identity: hex_lattice::ProcessorInstanceIdentity,
    fixture: &str,
) -> Binding {
    source_for_profile(control, identity, fixture, 8, None)
}
fn source_for_profile(
    control: &mut Control,
    identity: hex_lattice::ProcessorInstanceIdentity,
    fixture: &str,
    quantum: u16,
    restore: Option<hex_lattice_contract::NativeDeviceState>,
) -> Binding {
    let (build, source, mut preparation) =
        crate::candidate_custody::native_source_test_fixture::input_for(
            fixture,
            [
                0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
            ],
        );
    preparation.quantum = quantum;
    let binding = control
        .begin_native_source_preparation(
            identity,
            12,
            &build,
            source,
            preparation,
            restore,
            native::TrustUserCode::Acknowledged,
        )
        .unwrap();
    control
        .complete_native_source_preparation(
            binding,
            Some(hex_plugin_host::protocol::PresentationLatency {
                input_frames: 77,
                output_frames: 91,
            }),
        )
        .unwrap();
    control.start_native_effect(binding).unwrap();
    binding
}
fn recover(control: &mut Control, binding: Binding) -> crate::NativeSourcePreparationReturn {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        control.reclaim();
        if let Some(returned) = control.recover_native_source_preparation(binding) {
            return returned;
        }
        assert!(
            Instant::now() < deadline,
            "original endpoint/worker disposal"
        );
        std::thread::yield_now();
    }
}
fn allocation_free(call: impl FnOnce()) {
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(call);
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    {
        call();
    }
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_endpoint_moves_through_callback_and_returns_before_worker_disposal() {
    let _serial = serial();
    let (mut control, mut data, errors, mut retirement, binding) = setup();
    let state = control.native_source_prepared_state(binding).unwrap();
    control.publish_native_source_endpoint(binding).unwrap();
    allocation_free(|| {
        assert_eq!(
            control.publish_native_source_endpoint(binding),
            Err("native endpoint publication is busy")
        )
    });
    assert_eq!(
        control.native_source_prepared_state(binding),
        Some(state.clone())
    );
    control
        .with_native_effect_definition(&[binding], |definition| {
            use hex_lattice::{NativeDeviceDefinition, PreparedProcessorFactory};
            let tuning = hex_lattice::TuningTable::default();
            let context = hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &tuning,
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            };
            let factory = definition
                .prepare_instance_runtime_factory(identity(), &state, &context)
                .map_err(|e| e.to_string())?;
            let Some(PreparedProcessorFactory::Foreign(prepared)) = factory else {
                return Err("original foreign factory required".into());
            };
            assert_eq!(prepared.binding(), Some(binding));
            Ok(())
        })
        .unwrap();
    allocation_free(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            assert!(callback.has_native_source_endpoints());
        })
    });
    assert!(control.retire_native_source_preparation(binding));
    control.reclaim();
    assert_eq!(
        control.native_source_preparation_progress(binding),
        Some(crate::NativeSourcePreparationProgress::Retiring)
    );
    assert!(control.recover_native_source_preparation(binding).is_none());
    // Fill the actual return ring with real, control-constructed empty graph
    // owners. Pressure must leave this original endpoint in its callback cell.
    let callback = &mut data.owner_mut().unwrap().callback;
    while !callback.retired.is_full() {
        let graph = Playback::new(hex_engine::Project::new(48_000, vec![]).unwrap());
        assert!(
            callback
                .retired
                .push(ReturnedPublication::Legacy(Publication {
                    pcm_frames: 0,
                    forwarded_pcm: false,
                    revision: 0,
                    control_context_generation: 0,
                    explicit_control_context: false,
                    source_context: None,
                    policy: PublicationPolicy::InheritRuntime,
                    fresh_runtime_cancellation: None,
                    section_launch_frame: None,
                    streams: None,
                    playback: Box::new(graph),
                }))
                .is_ok()
        );
    }
    control.stop();
    allocation_free(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            assert!(callback.has_native_source_endpoints());
        })
    });
    assert_eq!(
        control.status.pending_stop_marker.load(Acquire),
        0,
        "return pressure must not delay priority Stop"
    );
    assert!(control.recover_native_source_preparation(binding).is_none());
    control.reclaim();
    allocation_free(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            assert!(!callback.has_native_source_endpoints());
        })
    });
    let returned = recover(&mut control, binding);
    let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
        panic!("original native reap")
    };
    assert!(disposal.status.success());
    assert_eq!(
        disposal.terminal.unwrap().blocks,
        0,
        "unvisited endpoint never fabricates DSP"
    );
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}
#[test]
#[cfg(feature = "allocation-probe")]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_installed_native_endpoint_requires_both_carriers_and_completed_backend_return() {
    let _serial = serial();
    for errors_first in [false, true] {
        let (mut control, mut data, errors, mut retirement, binding) = setup();
        let root = control.native_candidate_root();
        control.publish_native_source_endpoint(binding).unwrap();
        allocation_free(|| {
            data.owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
        drop(control);
        if errors_first {
            drop(errors);
            assert!(!retirement.advance().unwrap());
            drop(data);
        } else {
            drop(data);
            assert!(!retirement.advance().unwrap());
            drop(errors);
        }
        assert!(
            !retirement.advance().unwrap(),
            "returned guards alone cannot release an installed native endpoint"
        );
        assert!(
            crate::candidate_custody::native_source_test_fixture::original_worker_awaits_endpoint(
                &root, binding
            )
        );
        let mut backend = Some(());
        let mut pending = false;
        let mut disposed = false;
        assert!(
            crate::lifecycle::test_dispose_backend(
                &mut backend,
                &mut pending,
                &mut disposed,
                &mut retirement,
                |_| Ok(())
            )
            .unwrap()
        );
        finish(&mut retirement);
        assert!(
            retirement
                .root()
                .native_source_preparation_progress(binding)
                .is_none()
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn native_endpoint_return_visits_sources_after_a_vacant_cell() {
    let _serial = serial();
    let (mut control, mut data, errors, mut retirement, first) = setup();
    let second = source(
        &mut control,
        hex_lattice::ProcessorInstanceIdentity::new(hex_lattice::ProcessorChainOwner::Main, 3)
            .unwrap(),
    );
    for binding in [first, second] {
        control.publish_native_source_endpoint(binding).unwrap();
        allocation_free(|| {
            data.owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
    }
    for binding in [first, second] {
        assert!(control.retire_native_source_preparation(binding));
        allocation_free(|| {
            data.owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
        let returned = recover(&mut control, binding);
        assert!(matches!(
            returned.disposal,
            native::PreparationDisposal::Reaped(_)
        ));
        assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    }
    allocation_free(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| assert!(!callback.has_native_source_endpoints()))
    });
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}

#[cfg(feature = "allocation-probe")]
mod rendering;
