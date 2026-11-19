//! Actual Root source -> original queued endpoint -> closed claimed rendering
//! -> device host-format completion. Local claimed-render fixtures and actual
//! queue/ACK admission fixtures share original constructors and SDK owners.
use super::*;
use hex_lattice::{
    NativeDeviceDefinition, NativeDeviceRegistry, ProcessorChainOwner, ProcessorInstanceIdentity,
};
use hex_project::{Command, Created, Editor, Project};

mod deadlines;
mod epoch;
mod ordinary;
#[cfg(not(debug_assertions))]
mod profile;
mod reorder;
mod replacement;
mod stream_refusal;

fn close_backend(
    data: Guard<NativeCallbackOwner>,
    errors: Guard<ErrorCallbackOwner>,
    mut retirement: NativeRetirement,
) -> NativeRetirement {
    drop(data);
    drop(errors);
    assert!(!retirement.advance().unwrap());
    let mut retirement = std::thread::spawn(move || {
        let mut backend = Some(());
        let mut pending = false;
        let mut disposed = false;
        crate::lifecycle::test_dispose_backend(
            &mut backend,
            &mut pending,
            &mut disposed,
            &mut retirement,
            |_| Ok(()),
        )
        .unwrap();
        assert!(disposed && backend.is_none());
        retirement
    })
    .join()
    .unwrap();
    finish_source_carrier_return(&mut retirement);
    retirement
}

// Backend disposal may leave an exact endpoint in NativeRetirement when Root
// Work is busy. Keep polling that same owner before waiting for source recovery;
// this is not a claim that graph cleanup or worker retirement has completed.
fn finish_source_carrier_return(retirement: &mut NativeRetirement) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !retirement.test_native_source_carriers_returned() {
        retirement.advance().unwrap();
        assert!(Instant::now() < deadline, "native source carrier return");
        std::thread::yield_now();
    }
}

struct Fixture {
    control: Control,
    data: Guard<NativeCallbackOwner>,
    errors: Guard<ErrorCallbackOwner>,
    retirement: NativeRetirement,
    binding: Binding,
    unused: Option<Binding>,
    runtime: hex_engine::ClaimedAudioCandidateRuntime,
    anchors: [Option<hex_engine::AudioCandidateAnchors>; 1],
    samples: Vec<[f32; 2]>,
}
struct PreparedFixture {
    control: Control,
    data: Guard<NativeCallbackOwner>,
    errors: Guard<ErrorCallbackOwner>,
    retirement: NativeRetirement,
    binding: Binding,
    unused: Option<Binding>,
    owner: hex_engine::AudioCandidateOwner,
    project: Project,
    samples: Vec<[f32; 2]>,
}
fn fixture(location: usize, bypassed: bool) -> Fixture {
    let f = prepared_fixture(location, bypassed);
    let (mut runtime, anchor) = f.owner.split_constructor_on_control().ok().unwrap();
    runtime
        .apply_action(hex_engine::PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    Fixture {
        control: f.control,
        data: f.data,
        errors: f.errors,
        retirement: f.retirement,
        binding: f.binding,
        unused: f.unused,
        runtime,
        anchors: [Some(anchor)],
        samples: f.samples,
    }
}
fn prepared_fixture(location: usize, bypassed: bool) -> PreparedFixture {
    prepared_fixture_with_source(location, bypassed, "processing")
}
fn prepared_fixture_with_source(location: usize, bypassed: bool, fixture: &str) -> PreparedFixture {
    prepared_fixture_with_profile(location, bypassed, fixture, 8, true)
}
fn prepared_fixture_with_profile(
    location: usize,
    bypassed: bool,
    fixture: &str,
    quantum: u16,
    retain_unvisited: bool,
) -> PreparedFixture {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let bus = if location == 1 {
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 1.0,
            })
            .unwrap();
        Some(bus)
    } else {
        None
    };
    let owner = match location {
        0 => ProcessorChainOwner::Track(track.value()),
        1 => ProcessorChainOwner::Return(bus.unwrap().value()),
        _ => ProcessorChainOwner::Main,
    };
    let identity = ProcessorInstanceIdentity::new(owner, editor.project().next_identity()).unwrap();
    let (mut control, mut data, errors, retirement, binding) =
        setup_for_identity_with_profile(identity, fixture, quantum);
    let state = control.native_source_prepared_state(binding).unwrap();
    let command = match location {
        0 => Command::InsertDevice {
            track,
            before: None,
            state,
        },
        1 => Command::InsertReturnDevice {
            return_track: bus.unwrap(),
            before: None,
            state,
        },
        _ => Command::InsertMainDevice {
            before: None,
            state,
        },
    };
    let Created::Device(device) = editor.execute(command).unwrap() else {
        panic!("device")
    };
    assert_eq!(device.value(), identity.device_id());
    editor
        .execute(Command::SetDeviceBypass { device, bypassed })
        .unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "original-memory.wav".into(),
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
    let mut samples = vec![[0.25, -0.5]; 32];
    samples[31] = [-0.125, 0.375];
    let bank = hex_engine::OwnedAudioInputs::try_new(
        vec![(
            asset,
            hex_engine::OwnedAudioSource::try_resident(48_000, samples.clone())
                .ok()
                .unwrap(),
        )],
        hex_engine::AudioStorageReservation {
            allocation_items: 1024,
            handle_obligations: 256,
            max_prepared_data_bytes: usize::MAX,
            max_construction_data_bytes: usize::MAX,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
    )
    .ok()
    .unwrap();
    let owner = control
        .with_native_effect_definition(&[binding], |definition| {
            let definitions: [&dyn NativeDeviceDefinition; 1] = [definition];
            let registry = NativeDeviceRegistry::new(&definitions).unwrap();
            let mut custody = hex_engine::AudioPreparationCustody::try_new_on_control().unwrap();
            let token = custody
                .begin_input_on_control(
                    hex_engine::AudioArrangementInput::new(editor.project(), 11, bank)
                        .with_audio_prepared_revision()
                        .with_effect_registry(&registry),
                )
                .ok()
                .unwrap();
            let mut owner = hex_engine::AudioCandidateOwner::try_new_on_control(custody, token)
                .ok()
                .unwrap();
            assert!(owner.requires_effect_graph_custody_on_control().unwrap());
            let summary = owner
                .seal_constructor_on_control(hex_engine::AudioStorageLedgerReservation {
                    data_rows: 4096,
                    handle_rows: 2048,
                    root_rows: 8,
                    max_retained_bytes: usize::MAX,
                    max_construction_peak_bytes: usize::MAX,
                })
                .unwrap();
            assert_eq!(summary.data.unknown_storage, 0);
            Ok(owner)
        })
        .unwrap();
    let original = owner.identity();
    let refused = control
        .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 41)
        .err()
        .expect("unsplit source cannot publish a graph without its endpoint");
    assert_eq!(refused.reason, "native graph source is unavailable");
    assert_eq!(refused.owner.identity(), original);
    assert_eq!(control.submitted_revision(), 0);
    let owner = refused.owner;
    // Install the greater binding first, then insert the real graph's source
    // before it. The retained source must stay unvisited and return with0 DSP.
    let unused = (retain_unvisited && location == 2 && !bypassed).then(|| {
        let unused = source_for_profile(
            &mut control,
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 500).unwrap(),
            "processing",
            quantum,
            None,
        );
        control.publish_native_source_endpoint(unused).unwrap();
        allocation_free(|| {
            data.owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
        unused
    });
    control.publish_native_source_endpoint(binding).unwrap();
    allocation_free(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
        })
    });
    PreparedFixture {
        control,
        data,
        errors,
        retirement,
        binding,
        unused,
        owner,
        project: editor.project().clone(),
        samples,
    }
}
fn dispose(mut f: Fixture, expected_blocks: u64) {
    f.runtime
        .dispose_on_control()
        .unwrap()
        .record_anchor_cells_on_control(&mut f.anchors)
        .unwrap();
    let anchor = f.anchors[0].as_mut().unwrap();
    anchor.finish_disposal_on_control().unwrap();
    assert!(anchor.recover_inputs_on_control().is_some());
    assert!(f.control.retire_native_source_preparation(f.binding));
    if let Some(unused) = f.unused {
        assert!(f.control.retire_native_source_preparation(unused));
    }
    allocation_free(|| {
        f.data
            .owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    if f.unused.is_some() {
        // The real return queue has one cell. Drain the first original return
        // before asking the callback to return the second retained endpoint.
        f.control.reclaim();
        allocation_free(|| {
            f.data
                .owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
    }
    let returned = recover(&mut f.control, f.binding);
    let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
        panic!("native reap")
    };
    assert!(disposal.status.success());
    let terminal = disposal.terminal.unwrap();
    assert_eq!(
        (terminal.blocks, terminal.frames),
        (expected_blocks, expected_blocks * 8)
    );
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    if let Some(unused) = f.unused {
        // The first return removed the smaller route; the remaining original
        // owner still occupies its real sparse cell and has no fabricated work.
        allocation_free(|| {
            f.data
                .owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
        let returned = recover(&mut f.control, unused);
        let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
            panic!("unused native reap")
        };
        assert!(disposal.status.success());
        let terminal = disposal.terminal.unwrap();
        assert_eq!((terminal.blocks, terminal.frames), (0, 0));
        assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    }
    drop(f.control);
    drop(f.data);
    drop(f.errors);
    finish(&mut f.retirement);
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_endpoint_bank_renders_closed_track_return_main_at_final_host_acceptance() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    for location in 0..3 {
        for bypassed in [false, true] {
            let mut f = fixture(location, bypassed);
            let root = f.control.native_candidate_root();
            let mut output = [[0.0; 2]; 64];
            let mut committed = 0;
            for chunk in output.chunks_mut(5) {
                let mut host = [0.0_f32; 10];
                allocation_free(|| {
                    f.data.owner_mut().unwrap().run(|callback, _| {
                        callback.begin_block();
                        assert_eq!(callback.native_effect_quantum(), Some(8));
                        assert!(crate::device::valid_transport_buffer(
                            callback,
                            chunk.len() * 2,
                            2
                        ));
                        assert!(!crate::device::valid_transport_buffer(callback, 18, 2));
                        assert!(!crate::device::valid_transport_buffer(callback, 3, 2));
                        callback.begin_native_effect_callback(chunk.len()).unwrap();
                        let split = chunk.len().min(2);
                        let rendered = {
                            let mut window = crate::prepared::NativeEffectWindow::new(
                                &mut callback.native_sources,
                                0,
                            );
                            f.runtime.render_with_foreign_processors(
                                &mut chunk[..split],
                                None,
                                None,
                                None,
                                &mut window,
                            )
                        }
                        .and_then(|()| {
                            let mut window = crate::prepared::NativeEffectWindow::new(
                                &mut callback.native_sources,
                                split,
                            );
                            f.runtime.render_with_foreign_processors(
                                &mut chunk[split..],
                                None,
                                None,
                                None,
                                &mut window,
                            )
                        });
                        assert!(rendered.is_ok());
                        assert_eq!(
                            callback.native_sources.test_committed_frames(f.binding),
                            Some(committed)
                        );
                        assert!(crate::device::finish_host_chunk(
                            callback,
                            &mut host[..chunk.len() * 2],
                            chunk,
                            2,
                            &rendered,
                            committed,
                            Instant::now()
                        ));
                        assert_eq!(
                            callback.native_sources.test_committed_frames(f.binding),
                            Some(committed + chunk.len() as u64)
                        );
                        if let Some(unused) = f.unused {
                            assert_eq!(
                                callback.native_sources.test_committed_frames(unused),
                                Some(0)
                            );
                        }
                    })
                });
                for (actual, expected) in host[..chunk.len() * 2].chunks_exact(2).zip(chunk.iter())
                {
                    assert_eq!(actual, expected);
                }
                committed += chunk.len() as u64;
                let deadline = Instant::now() + Duration::from_secs(3);
                while crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(&root, f.binding).unwrap() < committed / 8 {
                    assert!(Instant::now() < deadline, "actual original Root worker completion");
                    std::thread::yield_now();
                }
            }
            let mut expected = [[0.0; 2]; 64];
            for (index, frame) in f.samples.iter().enumerate() {
                let gain = (if bypassed {
                    1.0
                } else {
                    0.5 + (index / 8) as f32 * 0.125
                }) + if location == 1 { 1.0 } else { 0.0 };
                expected[index + 16] = [frame[0] * gain, frame[1] * gain];
            }
            assert_eq!(
                output, expected,
                "Root endpoint location {location}, bypass {bypassed}"
            );
            dispose(f, 8);
        }
    }
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_endpoint_bank_refuses_final_conversion_and_late_stop_without_dispatch_or_replay() {
    let _serial = serial();
    for stop in [false, true] {
        let mut f = fixture(2, false);
        let callback = &mut f.data.owner_mut().unwrap().callback;
        let mut source = [[0.0; 2]; 8];
        callback.begin_block();
        callback.begin_native_effect_callback(8).unwrap();
        let rendered = f.runtime.render_with_foreign_processors(
            &mut source,
            None,
            None,
            None,
            &mut callback.native_sources,
        );
        assert!(rendered.is_ok());
        assert_eq!(
            callback.native_sources.test_committed_frames(f.binding),
            Some(0)
        );
        if stop {
            f.control.stop();
        } else {
            source[7][0] = f32::NAN;
        }
        let mut host = [9.0_f32; 16];
        allocation_free(|| {
            let valid = crate::device::finish_host_chunk(
                callback,
                &mut host,
                &mut source,
                2,
                &rendered,
                0,
                Instant::now(),
            );
            assert_eq!(valid, stop);
            assert_eq!(host, [0.0; 16]);
            assert_eq!(
                callback.native_sources.test_committed_frames(f.binding),
                Some(0)
            );
            assert!(callback.native_effects_revoked());
        });
        callback.begin_block();
        callback.begin_native_effect_callback(8).unwrap();
        let mut replay = [[9.0; 2]; 8];
        assert!(
            f.runtime
                .render_with_foreign_processors(
                    &mut replay,
                    None,
                    None,
                    None,
                    &mut callback.native_sources
                )
                .is_err()
        );
        assert_eq!(
            callback.native_sources.test_committed_frames(f.binding),
            Some(0)
        );
        dispose(f, 0);
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_graph_admission_retains_original_source_until_anchor_recovery() {
    let _serial = serial();
    for location in 0..3 {
        for bypassed in [false, true] {
            let PreparedFixture {
                mut control,
                mut data,
                errors,
                mut retirement,
                binding,
                unused,
                owner,
                samples,
                ..
            } = prepared_fixture(location, bypassed);
            let root = control.native_candidate_root();
            let ticket = control
                .stage_eligible_audio_candidate(
                    owner,
                    vec![],
                    PublicationPolicy::InheritRuntime,
                    41,
                )
                .unwrap_or_else(|r| panic!("actual native graph admission: {}", r.reason));
            assert_eq!(control.status.applied_revision.load(Acquire), 0);
            // Retirement refuses new graph uses, but the accepted original
            // anchors retain the actual endpoint and worker through rendering.
            assert!(control.retire_native_source_preparation(binding));
            control.command(PlaybackCommand::Play).unwrap();
            let mut output = [[0.0; 2]; 64];
            let mut committed = 0;
            for chunk in output.chunks_mut(8) {
                let mut host = [0.0_f32; 16];
                allocation_free(|| {
                    data.owner_mut().unwrap().run(|callback, _| {
                        callback.begin_block();
                        assert_eq!(callback.native_effect_quantum(), Some(8));
                        callback.begin_native_effect_callback(chunk.len()).unwrap();
                        let rendered = callback.render(chunk);
                        assert!(rendered.is_ok(), "actual installed graph: {rendered:?}");
                        assert!(crate::device::finish_host_chunk(
                            callback,
                            &mut host[..chunk.len() * 2],
                            chunk,
                            2,
                            &rendered,
                            committed,
                            Instant::now()
                        ));
                    })
                });
                assert_eq!(control.status.applied_revision.load(Acquire), 1);
                committed += chunk.len() as u64;
                let deadline = Instant::now() + Duration::from_secs(3);
                while crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(&root, binding).unwrap() < committed / 8 {
                    assert!(Instant::now() < deadline, "actual Root worker completion");
                    std::thread::yield_now();
                }
                control.reclaim();
                assert!(control.recover_native_source_preparation(binding).is_none());
            }
            let mut expected = [[0.0; 2]; 64];
            for (index, frame) in samples.iter().enumerate() {
                let gain = (if bypassed {
                    1.0
                } else {
                    0.5 + (index / 8) as f32 * 0.125
                }) + if location == 1 { 1.0 } else { 0.0 };
                expected[index + 16] = [frame[0] * gain, frame[1] * gain];
            }
            assert_eq!(
                output, expected,
                "admitted original graph location {location}, bypass {bypassed}"
            );
            drop(data);
            drop(errors);
            // Both original carriers and the existing completed-backend test
            // frontier are required before extracting the original active graph.
            assert!(!retirement.advance().unwrap());
            retirement = std::thread::spawn(move || {
                let mut backend = Some(());
                let mut pending = false;
                let mut disposed = false;
                let _ = crate::lifecycle::test_dispose_backend(
                    &mut backend,
                    &mut pending,
                    &mut disposed,
                    &mut retirement,
                    |_| Ok(()),
                )
                .unwrap();
                assert!(disposed && backend.is_none());
                retirement
            })
            .join()
            .unwrap();
            let recovered = crate::prepared::candidate_tests::recover(&mut control, &ticket);
            assert_eq!(
                recovered.return_kind,
                crate::AudioCandidateReturnKind::FinalActive
            );
            drop(recovered);
            let returned = recover(&mut control, binding);
            assert!(returned.error.is_none() && returned.cleanup_error.is_none());
            let native::PreparationDisposal::Reaped(terminal) = returned.disposal else {
                panic!("original native disposal")
            };
            assert!(terminal.status.success());
            let terminal = terminal.terminal.unwrap();
            assert_eq!((terminal.blocks, terminal.frames), (8, 64));
            if let Some(unused) = unused {
                // The actual completed-backend frontier already closes Root
                // and requests retirement of every retained source.
                let returned = recover(&mut control, unused);
                assert!(returned.error.is_none() && returned.cleanup_error.is_none());
                let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
                    panic!("original unvisited source disposal")
                };
                let terminal = disposal.terminal.unwrap();
                assert_eq!((terminal.blocks, terminal.frames), (0, 0));
            }
            drop(control);
            finish(&mut retirement);
        }
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn cancelled_actual_native_graph_holds_endpoint_until_original_report_recovery() {
    let _serial = serial();
    let PreparedFixture {
        mut control,
        mut data,
        errors,
        mut retirement,
        binding,
        unused,
        owner,
        ..
    } = prepared_fixture(0, false);
    assert!(unused.is_none());
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 41)
        .unwrap_or_else(|r| panic!("actual native graph: {}", r.reason));
    assert!(control.retire_native_source_preparation(binding));
    assert_eq!(
        control.cancel_audio_candidate(&ticket),
        crate::AudioCandidateCancellationResult::Cancelled
    );
    allocation_free(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            assert_eq!(
                callback.native_effect_quantum(),
                Some(8),
                "original anchors still hold the endpoint"
            );
        })
    });
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    let recovered = crate::prepared::candidate_tests::recover(&mut control, &ticket);
    assert_eq!(
        recovered.return_kind,
        crate::AudioCandidateReturnKind::NeverInstalled
    );
    drop(recovered);
    allocation_free(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    let returned = recover(&mut control, binding);
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
        panic!("actual native disposal")
    };
    assert!(disposal.status.success());
    let terminal = disposal.terminal.unwrap();
    assert_eq!((terminal.blocks, terminal.frames), (0, 0));
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}

fn replacement_preparation(
    control: &mut Control,
    bindings: &[Binding],
    project: &Project,
    samples: &[[f32; 2]],
) -> crate::AudioCandidateTicket {
    replacement_preparation_inner(control, bindings, project, samples, None, vec![], None)
}
fn replacement_epoch_preparation(
    control: &mut Control,
    bindings: &[Binding],
    project: &Project,
    samples: &[[f32; 2]],
    fence: StoppedEditFence,
) -> crate::AudioCandidateTicket {
    replacement_preparation_inner(
        control,
        bindings,
        project,
        samples,
        Some(fence),
        vec![],
        None,
    )
}
fn replacement_preparation_inner(
    control: &mut Control,
    bindings: &[Binding],
    project: &Project,
    samples: &[[f32; 2]],
    fence: Option<StoppedEditFence>,
    domains: Vec<crate::AudioCandidateDomainToken>,
    streamed: Option<hex_engine::StreamedSourceMetadata>,
) -> crate::AudioCandidateTicket {
    let quote = control
        .with_native_effect_definition(bindings, |definition| {
            let definitions: [&dyn NativeDeviceDefinition; 1] = [definition];
            let registry = NativeDeviceRegistry::new(&definitions).unwrap();
            Ok(hex_engine::AudioCandidateEffects::new(&registry)
                .quote(project, 1)
                .unwrap())
        })
        .unwrap();
    let token = control
        .reserve_audio_candidate_preparation(
            crate::AudioCandidatePreparationReservation {
                source_capacity: 1,
                member_rows: quote.member_rows,
                transfer_rows: quote.transfer_rows,
                handle_rows: quote.handle_rows,
                storage: quote
                    .storage_reservation(std::mem::size_of_val(samples))
                    .unwrap(),
            },
            domains,
        )
        .unwrap_or_else(|refusal| {
            panic!("reserved replacement: {} quote {quote:?}", refusal.reason)
        });
    // A binding from another Root cannot select any source here. Incrementing
    // an epoch would accidentally select a real coexisting replacement with
    // the same device identity. Refusal must leave this destination untouched.
    if let Some(binding) = bindings.first() {
        let stale = Binding {
            root: binding.root.checked_add(1).unwrap(),
            ..*binding
        };
        assert!(
            control
                .prepare_audio_candidate_with_native_sources_on_control(token, &[stale], |_, _| {
                    panic!("stale source must not compile")
                })
                .is_err()
        );
    }
    let asset = project.assets().next().unwrap().id();
    let source = hex_engine::Audio::new(48_000, samples.to_vec()).unwrap();
    let mut preparation_error = None;
    assert_eq!(
        control
            .prepare_audio_candidate_with_native_sources_on_control(
                token,
                bindings,
                |cell, effects| {
                    if let Some(metadata) = streamed.as_ref() {
                        cell.add_streamed_source_on_control(metadata.clone())?;
                    } else {
                        cell.copy_resident_source_on_control(asset, &source)?;
                    }
                    effects
                        .prepare(
                            cell,
                            project,
                            12,
                            hex_engine::AudioSourceContext {
                                project_generation: 41,
                                editor_revision: 1,
                            },
                            None,
                        )
                        .inspect_err(|error| preparation_error = Some(*error))
                }
            )
            .unwrap(),
        crate::AudioCandidatePreparationProgress::Ready,
        "actual reserved preparation: {preparation_error:?}"
    );
    let ticket = if let Some(fence) = fence {
        assert!(
            control
                .stage_audio_candidate_preparation(token, 41)
                .is_err()
        );
        control
            .stage_native_epoch_preparation(token, fence)
            .unwrap()
    } else {
        control
            .stage_audio_candidate_preparation(token, 41)
            .unwrap()
    };
    assert!(
        control
            .prepare_audio_candidate_with_native_sources_on_control(token, bindings, |_, _| {
                panic!("staged token must not compile again")
            })
            .is_err()
    );
    ticket
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_track_return_main_bypass_handoff_keeps_original_endpoint_and_clock() {
    let _serial = serial();
    for location in 0..3 {
        assert_live_handoff(location, LiveEdit::Bypass);
    }
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_main_removal_drains_original_finite_echo_and_bridge_before_it_stops() {
    let _serial = serial();
    assert_live_handoff(2, LiveEdit::Remove);
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_infinite_main_tail_refuses_removal_without_changing_original_audio() {
    let _serial = serial();
    assert_live_handoff(2, LiveEdit::RefusedRemove);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_main_tail_survives_successors_and_retires_before_backend_shutdown() {
    let _serial = serial();
    let PreparedFixture {
        mut control,
        mut data,
        errors,
        mut retirement,
        binding,
        unused,
        owner,
        project,
        samples,
    } = prepared_fixture_with_source(2, false, "tail");
    let root = control.native_candidate_root();
    let mut tickets = vec![
        control
            .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 41)
            .unwrap_or_else(|r| panic!("first graph: {}", r.reason)),
    ];
    let mut editor = Editor::new(project, 8).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 64];
    for (block, chunk) in output.chunks_mut(8).enumerate() {
        if matches!(block, 1 | 2 | 6) {
            if block == 1 {
                editor
                    .execute(Command::RemoveDevice(
                        editor.project().main_devices()[0].id(),
                    ))
                    .unwrap();
            } else {
                // One successor carries the audible original tail. Another
                // follows its accepted drain, while the backend stays live.
                editor
                    .execute(Command::RenameTrack {
                        track: editor.project().tracks()[0].id(),
                        name: format!("successor {block}"),
                    })
                    .unwrap();
            }
            let bindings = if block == 1 {
                std::slice::from_ref(&binding)
            } else {
                &[]
            };
            tickets.push(replacement_preparation(
                &mut control,
                bindings,
                editor.project(),
                &samples,
            ));
        }
        let mut host = [0.0_f32; 16];
        allocation_free(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.begin_native_effect_callback(8).unwrap();
                let rendered = callback.render(chunk);
                assert!(rendered.is_ok(), "successor render: {rendered:?}");
                assert!(crate::device::finish_host_chunk(
                    callback,
                    &mut host,
                    chunk,
                    2,
                    &rendered,
                    block as u64 * 8,
                    Instant::now()
                ));
            })
        });
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            tickets.len() as u64
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        if block <= 6 {
            let expected_blocks = (block as u64 + 1).min(5);
            while crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(
                &root, binding,
            )
            .unwrap()
                < expected_blocks
            {
                assert!(Instant::now() < deadline, "original tail worker completion");
                std::thread::yield_now();
            }
            if block >= 4 {
                assert_eq!(
                    crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(&root, binding),
                    Some(5)
                );
            }
        }
        if block == 1 {
            while !control.retire_native_source_preparation(binding) {
                assert!(Instant::now() < deadline, "original retirement request");
                std::thread::yield_now();
            }
        }
        if block == 2 {
            let old = crate::prepared::candidate_tests::recover(&mut control, &tickets[1]);
            assert_eq!(old.return_kind, crate::AudioCandidateReturnKind::Replaced);
            drop(old);
        }
        if (1..6).contains(&block) {
            assert!(control.recover_audio_candidate(&tickets[0]).is_none());
            assert!(control.recover_native_source_preparation(binding).is_none());
        }
        if block == 6 {
            let old = crate::prepared::candidate_tests::recover(&mut control, &tickets[2]);
            assert_eq!(old.return_kind, crate::AudioCandidateReturnKind::Replaced);
            drop(old);
            let original = crate::prepared::candidate_tests::recover(&mut control, &tickets[0]);
            assert_eq!(
                original.return_kind,
                crate::AudioCandidateReturnKind::Replaced
            );
            drop(original);
        }
    }
    let mut expected = [[0.0; 2]; 64];
    expected[8..32].copy_from_slice(&samples[8..32]);
    for (i, frame) in samples[..8].iter().enumerate() {
        for channel in 0..2 {
            expected[i + 16][channel] += frame[channel] * 0.5;
            expected[i + 19][channel] += frame[channel] * 0.125;
        }
    }
    assert_eq!(
        output, expected,
        "carried original FIR and bridge remain exact"
    );
    let returned = recover(&mut control, binding);
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
        panic!("original source must reap while backend is live")
    };
    assert!(disposal.status.success());
    let terminal = disposal.terminal.unwrap();
    assert_eq!((terminal.blocks, terminal.frames), (5, 40));

    drop(data);
    drop(errors);
    assert!(!retirement.advance().unwrap());
    retirement = std::thread::spawn(move || {
        let mut backend = Some(());
        let mut pending = false;
        let mut disposed = false;
        crate::lifecycle::test_dispose_backend(
            &mut backend,
            &mut pending,
            &mut disposed,
            &mut retirement,
            |_| Ok(()),
        )
        .unwrap();
        assert!(disposed && backend.is_none());
        retirement
    })
    .join()
    .unwrap();
    let current = crate::prepared::candidate_tests::recover(&mut control, &tickets[3]);
    assert_eq!(
        current.return_kind,
        crate::AudioCandidateReturnKind::FinalActive
    );
    drop(current);
    if let Some(unused) = unused {
        let returned = recover(&mut control, unused);
        let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
            panic!("original unvisited source disposal")
        };
        let terminal = disposal.terminal.unwrap();
        assert_eq!((terminal.blocks, terminal.frames), (0, 0));
    }
    drop(control);
    finish(&mut retirement);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LiveEdit {
    Bypass,
    Remove,
    RefusedRemove,
}
fn assert_live_handoff(location: usize, edit: LiveEdit) {
    let PreparedFixture {
        mut control,
        mut data,
        errors,
        mut retirement,
        binding,
        unused,
        owner,
        project,
        samples,
    } = prepared_fixture_with_source(
        location,
        false,
        match edit {
            LiveEdit::Remove => "tail",
            LiveEdit::RefusedRemove => "infinite",
            LiveEdit::Bypass => "processing",
        },
    );
    let root = control.native_candidate_root();
    let first = control
        .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 41)
        .unwrap_or_else(|r| panic!("first graph: {}", r.reason));
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 64];
    let mut second = None;
    let mut committed = 0;
    for (block, chunk) in output.chunks_mut(8).enumerate() {
        if block == 1 {
            let device = match location {
                0 => project.tracks()[0].devices()[0].id(),
                1 => project.return_tracks()[0].devices()[0].id(),
                _ => project.main_devices()[0].id(),
            };
            let mut editor = Editor::new(project.clone(), 8).unwrap();
            editor
                .execute(match edit {
                    LiveEdit::Bypass => Command::SetDeviceBypass {
                        device,
                        bypassed: true,
                    },
                    LiveEdit::Remove | LiveEdit::RefusedRemove => Command::RemoveDevice(device),
                })
                .unwrap();
            second = Some(replacement_preparation(
                &mut control,
                &[binding],
                editor.project(),
                &samples,
            ));
        }
        let mut host = [0.0_f32; 16];
        allocation_free(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.begin_native_effect_callback(8).unwrap();
                let rendered = callback.render(chunk);
                assert!(rendered.is_ok(), "installed replacement: {rendered:?}");
                assert!(crate::device::finish_host_chunk(
                    callback,
                    &mut host,
                    chunk,
                    2,
                    &rendered,
                    committed,
                    Instant::now()
                ));
            })
        });
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            if block == 0 || edit == LiveEdit::RefusedRemove {
                1
            } else {
                2
            }
        );
        committed += 8;
        let deadline = Instant::now() + Duration::from_secs(3);
        let expected_blocks = if edit == LiveEdit::Remove {
            (committed / 8).min(5)
        } else {
            committed / 8
        };
        while
            crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(
                &root, binding,
            )
            .unwrap()
                < expected_blocks
        {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        if block == 1 {
            if edit == LiveEdit::RefusedRemove {
                let refused = crate::prepared::candidate_tests::recover(
                    &mut control,
                    second.as_ref().unwrap(),
                );
                assert_eq!(
                    refused.return_kind,
                    crate::AudioCandidateReturnKind::NeverInstalled
                );
                drop(refused);
                assert!(
                    control.recover_audio_candidate(&first).is_none(),
                    "original graph remains active"
                );
            } else if edit == LiveEdit::Bypass {
                let old = crate::prepared::candidate_tests::recover(&mut control, &first);
                assert_eq!(old.return_kind, crate::AudioCandidateReturnKind::Replaced);
                drop(old);
            } else {
                assert!(
                    control.recover_audio_candidate(&first).is_none(),
                    "captured tail retains its original anchor"
                );
            }
            // An accepted graph still references the original source;
            // asking retirement must still preserve its physical endpoint.
            let deadline = Instant::now() + Duration::from_secs(3);
            while !control.retire_native_source_preparation(binding) {
                assert!(
                    Instant::now() < deadline,
                    "original retirement control remained busy"
                );
                assert!(!matches!(
                    control.native_source_preparation_progress(binding),
                    Some(
                        crate::NativeSourcePreparationProgress::Failed
                            | crate::NativeSourcePreparationProgress::Complete
                    )
                ));
                std::thread::yield_now();
            }
        }
    }
    let mut expected = [[0.0; 2]; 64];
    let gain = if location == 1 { 2.0 } else { 1.0 };
    if edit == LiveEdit::Remove {
        assert_eq!(
            location, 2,
            "this oracle covers the Main clock-preserving cut"
        );
        expected[8..32].copy_from_slice(&samples[8..32]);
        for (i, frame) in samples[..8].iter().enumerate() {
            for channel in 0..2 {
                expected[i + 16][channel] += frame[channel] * 0.5;
                expected[i + 19][channel] += frame[channel] * 0.125;
            }
        }
        assert_eq!(
            crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(
                &root, binding
            ),
            Some(5)
        );
    } else {
        for (i, frame) in samples.iter().enumerate() {
            let scale = if edit == LiveEdit::RefusedRemove {
                0.5 + (i / 8) as f32 * 0.125
            } else {
                gain
            };
            expected[i + 16] = [frame[0] * scale, frame[1] * scale];
        }
    }
    assert_eq!(output, expected, "actual handoff location {location}");
    drop(data);
    drop(errors);
    assert!(!retirement.advance().unwrap());
    retirement = std::thread::spawn(move || {
        let mut backend = Some(());
        let mut pending = false;
        let mut disposed = false;
        let _ = crate::lifecycle::test_dispose_backend(
            &mut backend,
            &mut pending,
            &mut disposed,
            &mut retirement,
            |_| Ok(()),
        )
        .unwrap();
        assert!(disposed && backend.is_none());
        retirement
    })
    .join()
    .unwrap();
    if edit == LiveEdit::Remove {
        let old = crate::prepared::candidate_tests::recover(&mut control, &first);
        assert_eq!(old.return_kind, crate::AudioCandidateReturnKind::Replaced);
        drop(old);
    }
    let current_ticket = if edit == LiveEdit::RefusedRemove {
        first
    } else {
        second.unwrap()
    };
    let current = crate::prepared::candidate_tests::recover(&mut control, &current_ticket);
    assert_eq!(
        current.return_kind,
        crate::AudioCandidateReturnKind::FinalActive
    );
    drop(current);
    let returned = recover(&mut control, binding);
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
        panic!("actual source")
    };
    assert!(disposal.status.success());
    let terminal = disposal.terminal.unwrap();
    let blocks = if edit == LiveEdit::Remove { 5 } else { 8 };
    assert_eq!((terminal.blocks, terminal.frames), (blocks, blocks * 8));
    if let Some(unused) = unused {
        let _ = recover(&mut control, unused);
    }
    drop(control);
    finish(&mut retirement);
}
