//! Real source/worker and actual callback-carrier transfer; no Hex app/device.
use super::*;
use crate::candidate_custody::native_source_test_fixture::{identity, source_reservation};
use crate::native_callback_owner::{
    ErrorCallbackOwner, Guard, NativeCallbackOwner, NativeRetirement,
};
use crate::prepared::candidate_tests::{finish, serial};
use std::time::{Duration, Instant};

#[test]
fn stopped_frontier_preserves_only_actual_quantum_and_empty_receipt() {
    use crate::plugin_worker::CommittedQuantumState;
    use hex_engine::ForeignProgramFrontier as Frontier;
    let key = std::num::NonZeroU64::new(7).unwrap();
    for (quantum, expected_key) in [
        (CommittedQuantumState::Closed, None),
        (CommittedQuantumState::OpenUnprogrammed, None),
        (CommittedQuantumState::OpenProgram(key), Some(key)),
    ] {
        for worker in [Frontier::Unknown, Frontier::Empty, Frontier::Program(key)] {
            let observed = hex_plugin_host::transport::QuantumObservation::Committed(
                crate::plugin_worker::CommittedQuantum {
                    binding: Binding {
                        root: 1,
                        instance: 2,
                        graph: 3,
                        epoch: 4,
                    },
                    state: quantum,
                },
            );
            assert_eq!(
                stopped_program_frontier(observed, worker),
                Frontier::Stopped {
                    open_quantum: expected_key,
                    worker_empty: worker == Frontier::Empty,
                }
            );
        }
    }
    let binding = Binding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    };
    assert_eq!(
        stopped_program_frontier(
            hex_plugin_host::transport::QuantumObservation::Untouched(binding),
            Frontier::Unknown
        ),
        Frontier::Stopped {
            open_quantum: None,
            worker_empty: false
        }
    );
    assert_eq!(
        stopped_program_frontier(
            hex_plugin_host::transport::QuantumObservation::Untouched(binding),
            Frontier::Empty
        ),
        Frontier::Stopped {
            open_quantum: None,
            worker_empty: true
        }
    );
}

#[test]
fn committed_quantum_is_unavailable_without_native_custody_or_after_closing() {
    let (mut control, _callback) = channel(48_000).unwrap();
    let binding = Binding {
        root: 1,
        instance: 1,
        graph: 1,
        epoch: 1,
    };
    assert!(
        control
            .publication
            .try_native_source_committed_quantum_on_control(binding)
            .is_err()
    );
    control.publication.enable_native_custody().unwrap();
    control
        .publication
        .status
        .closing
        .store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(
        control
            .publication
            .try_native_source_committed_quantum_on_control(binding),
        Err("native automation output is closing")
    );
}

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
        .configure_plugin_callback_quantum(&mut callback, quantum)
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
    let runtime_slots = 3;
    // The public preparation quote includes the owned-runtime ceiling even for
    // this foreign-only graph. Reserve that current ceiling per runtime cell,
    // in addition to the fixture's existing source/compiler overlap allowance.
    // This is caller capacity, not allocated memory or a relaxed admission rule.
    let fixture_data_bytes = (192 * 1024 * 1024_usize)
        .checked_add(
            hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
                .checked_mul(runtime_slots)
                .unwrap(),
        )
        .unwrap();
    control
        .configure_audio_candidate_publication(crate::AudioCandidatePublicationReservation {
            runtime_slots,
            archive_slots: 3,
            member_rows: rows.member_rows.checked_mul(3).unwrap(),
            transfer_rows: rows.transfer_rows.checked_mul(3).unwrap(),
            receipt_rows: 4,
            data_rows: 4096,
            handle_rows: 4096,
            // Two original native-source peak reservations coexist with the
            // full effect-aware revision quote during rooted preparation.
            max_retained_data_bytes: fixture_data_bytes,
            max_construction_peak_data_bytes: fixture_data_bytes,
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
    source_for_profile_with_authored(control, identity, fixture, quantum, restore, &[])
}

fn source_for_profile_with_authored(
    control: &mut Control,
    identity: hex_lattice::ProcessorInstanceIdentity,
    fixture: &str,
    quantum: u16,
    restore: Option<hex_lattice_contract::NativeDeviceState>,
    ids: &[u32],
) -> Binding {
    source_for_profile_with_authored_mode(
        control,
        identity,
        fixture,
        quantum,
        restore,
        ids,
        crate::plugin_effects::PluginProfile::Effect,
    )
}

fn source_for_profile_with_authored_mode(
    control: &mut Control,
    identity: hex_lattice::ProcessorInstanceIdentity,
    fixture: &str,
    quantum: u16,
    restore: Option<hex_lattice_contract::NativeDeviceState>,
    ids: &[u32],
    mode: crate::plugin_effects::PluginProfile,
) -> Binding {
    let (build, source, mut preparation) =
        crate::candidate_custody::native_source_test_fixture::input_for(
            fixture,
            [
                0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
            ],
        );
    preparation.quantum = quantum;
    assert_eq!(control.plugin_callback_quantum_on_control(), Some(quantum));
    let identity_bytes = source.identity();
    let executable_digest = hex_digest(identity_bytes.binary.executable_sha256);
    let info_digest = hex_digest(identity_bytes.binary.info_sha256);
    let path = fixture_path(fixture);
    let hosting = crate::plugin_effects::Hosting::from_test_build(build, preparation.limits);
    let hosting = if std::env::var_os("HEX_ORDINARY_GROUP_EDITOR_MAIN_DRIVER").is_some() {
        hosting.with_grouped_editor_callbacks()
    } else {
        hosting
    };
    let binding = hosting
        .begin_on_control(
            control,
            crate::plugin_effects::EffectSelection {
                module: &path,
                executable: "Contents/MacOS/fixture",
                cid: identity_bytes.class.bytes(),
                executable_sha256: &executable_digest,
                info_sha256: &info_digest,
            },
            identity.owner(),
            identity.device_id(),
            12,
            48_000,
            mode,
            restore,
        )
        .unwrap();
    hosting
        .complete_on_control(
            control,
            binding,
            Some(crate::plugin_effects::EffectPresentation {
                // Explicit headless fixture inputs, not production/device measurements.
                input_frames: 77,
                output_frames: 91,
            }),
        )
        .unwrap();
    let authored = crate::plugin_effects::AuthoredSelection::from_targets(
        ids.iter()
            .map(|id| (*id, Some(identity_bytes.class.bytes()))),
    )
    .unwrap();
    control
        .start_native_effect_with_authored_selection(binding, authored.as_ref())
        .unwrap();
    binding
}
fn hex_digest(bytes: [u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn fixture_path(fixture: &str) -> std::path::PathBuf {
    let receipt = std::path::PathBuf::from(std::env::var_os("HEX_VST3_MACOS_RECEIPT").unwrap());
    receipt.parent().unwrap().join(format!(
        "hex-vst3-host.app/Contents/Resources/fixtures/{fixture}.vst3"
    ))
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
fn actual_selected_effect_revalidation_refuses_changed_metadata_before_root_reservation() {
    let _serial = serial();
    let (mut control, data, errors, mut retirement, original) = setup();
    let original_state = control.native_source_prepared_state(original).unwrap();
    let (build, source, preparation) =
        crate::candidate_custody::native_source_test_fixture::input();
    let identity = source.identity();
    let executable = hex_digest(identity.binary.executable_sha256);
    let info = hex_digest(identity.binary.info_sha256);
    let path = fixture_path("processing");
    let hosting = crate::plugin_effects::Hosting::from_test_build(build, preparation.limits);
    let wrong = "0".repeat(64);
    assert_ne!(executable, wrong);
    assert_ne!(info, wrong);
    let missing = path.join("missing.vst3");
    for case in 0..5 {
        let selected = crate::plugin_effects::EffectSelection {
            module: if case == 4 { &missing } else { &path },
            executable: if case == 3 {
                "Contents/MacOS/missing"
            } else {
                "Contents/MacOS/fixture"
            },
            cid: if case == 2 {
                [0; 16]
            } else {
                identity.class.bytes()
            },
            executable_sha256: if case == 0 { &wrong } else { &executable },
            info_sha256: if case == 1 { &wrong } else { &info },
        };
        assert!(
            hosting
                .begin_on_control(
                    &mut control,
                    selected,
                    hex_engine::ProcessorChainOwner::Main,
                    3,
                    12,
                    48_000,
                    crate::plugin_effects::PluginProfile::Effect,
                    None,
                )
                .is_err()
        );
        assert_eq!(
            control.native_source_prepared_state(original),
            Some(original_state.clone())
        );
    }
    let next = source_for_fixture(
        &mut control,
        hex_lattice::ProcessorInstanceIdentity::new(hex_engine::ProcessorChainOwner::Main, 3)
            .unwrap(),
        "processing",
    );
    assert_eq!(
        next.epoch,
        original.epoch + 1,
        "refused selections must not reserve native cells or start a child"
    );
    for binding in [original, next] {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !control.retire_native_source_preparation(binding) {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let returned = recover(&mut control, binding);
        assert!(returned.error.is_none() && returned.cleanup_error.is_none());
        assert!(
            matches!(returned.disposal, native::PreparationDisposal::Reaped(ref retired) if retired.status.success())
        );
    }
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_endpoint_moves_through_callback_and_returns_before_worker_disposal() {
    let _serial = serial();
    let (mut control, mut data, errors, mut retirement, binding) = setup();
    assert!(control.has_vacant_native_source_slots_on_control(1));
    assert!(!control.has_vacant_native_source_slots_on_control(2));
    let state = control.native_source_prepared_state(binding).unwrap();
    assert_ne!(
        control.native_source_endpoint_installed(binding),
        Some(true)
    );
    control.publish_native_source_endpoint(binding).unwrap();
    assert_ne!(
        control.native_source_endpoint_installed(binding),
        Some(true),
        "queueing an original endpoint is not callback installation"
    );
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
    let deadline = Instant::now() + Duration::from_secs(3);
    while control.native_source_endpoint_installed(binding) != Some(true) {
        assert!(Instant::now() < deadline, "actual original endpoint ACK");
        std::thread::yield_now();
    }
    assert_eq!(
        control.native_source_endpoint_installed(Binding {
            epoch: binding.epoch + 1,
            ..binding
        }),
        None
    );
    assert!(control.retire_native_source_preparation(binding));
    assert_ne!(
        control.native_source_endpoint_installed(binding),
        Some(true)
    );
    control.reclaim();
    // This query is nonblocking: the lifecycle worker can briefly hold Bank.
    // Observe a status before asserting retirement, without returning the
    // original callback endpoint or accepting a completed/failed source.
    let deadline = Instant::now() + Duration::from_secs(3);
    let progress = loop {
        if let Some(progress) = control.native_source_preparation_progress(binding) {
            break progress;
        }
        assert!(Instant::now() < deadline, "native source retirement status");
        std::thread::yield_now();
    };
    assert_eq!(progress, crate::NativeSourcePreparationProgress::Retiring);
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
                    quantized_seek_frame: None,
                    streams: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_continuation: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_references: None,
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
        // Backend disposal supplies its actual frontier once; native return
        // and cleanup may still be pending on the existing Work lane. Preserve
        // all owners while driving the same supported retirement operation.
        let deadline = Instant::now() + Duration::from_secs(3);
        while !crate::lifecycle::test_dispose_backend(
            &mut backend,
            &mut pending,
            &mut disposed,
            &mut retirement,
            |_| Ok(()),
        )
        .unwrap()
        {
            assert!(Instant::now() < deadline, "original native retirement");
            std::thread::yield_now();
        }
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
