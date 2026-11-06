//! Actual original SDK graph PCM, not app/Root admission or physical deadlines.
use super::*;
use crate::plugin_worker::native_tests::original_instance_for_binding;
use hex_lattice::{NativeDeviceDefinition, NativeDeviceRegistry, ProcessorChainOwner};
use hex_project::{Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_effect_data_limit_precedes_allocations_and_retains_original_disposal() {
    let binding = Binding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    };
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 2).unwrap();
    let diagnostic = hex_plugin_host::NATIVE_DIAGNOSTIC_DATA_BYTES;
    for allowance in [0, diagnostic - 1, diagnostic] {
        let fits = allowance == diagnostic;
        let instance = original_instance_for_binding(binding);
        let configuration = NativeEffect::configuration_on_control(identity, &instance).unwrap();
        let quoted = NativeWorker::preconstruction_data_bytes_on_control(&instance).unwrap()
            + configuration.prepared.total_latency_frames() as usize
                * std::mem::size_of::<[f32; 2]>()
            + EffectSourceMetadata::required_data_bytes(instance.configuration()).unwrap();
        drop(configuration);
        let limit = quoted + allowance;
        let mut outcome = None;
        let construct = || {
            outcome = Some(NativeEffect::prepare_retained_with_data_limit_on_control(
                identity, instance, limit,
            ));
        };
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(construct);
            if !fits {
                // Normal refusal may reclaim source config/capture on control,
                // but must not allocate dry/queues/Boxes or a joining context.
                assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
                assert!(measured.count_current <= 0 && measured.bytes_current <= 0);
            }
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            construct();
        }
        match outcome.unwrap() {
            Ok(mut effect) => {
                assert!(fits);
                assert_eq!(
                    effect
                        .constructor_storage_on_control()
                        .unwrap()
                        .data_bytes(),
                    Some(quoted)
                );
                let retired = effect.retire_on_control().unwrap();
                assert!(retired.failure.is_none());
                assert_eq!(retired.native.terminal.unwrap().blocks, 0);
            }
            Err(mut failure) => {
                assert!(!fits);
                assert_eq!(failure.binding(), binding);
                assert!(failure.to_string().contains("data reservation exceeded"));
                let deadline = Instant::now() + Duration::from_secs(3);
                loop {
                    if let Some(disposal) = failure.poll_disposal_on_control().unwrap() {
                        assert!(matches!(
                            disposal,
                            hex_plugin_host::native::PreparationDisposal::Reaped(_)
                        ));
                        break;
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::yield_now();
                }
            }
        }
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_effect_storage_counts_spare_dry_capacity_and_retains_the_original_state_alias() {
    let binding = Binding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    };
    let mut effect = NativeEffect::prepare_on_control(
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, binding.instance).unwrap(),
        original_instance_for_binding(binding),
    )
    .unwrap();
    let before = effect.constructor_storage_on_control().unwrap();
    let capacity = effect.callback.dry.capacity();
    effect.callback.dry.reserve_exact(17);
    let after = effect.constructor_storage_on_control().unwrap();
    assert_eq!(after.worker, before.worker);
    assert_eq!(after.state_alias_handles, 1);
    assert_eq!(
        after.data_bytes().unwrap() - before.data_bytes().unwrap(),
        (effect.callback.dry.capacity() - capacity) * std::mem::size_of::<[f32; 2]>()
    );
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(|| {
            assert_eq!(effect.constructor_storage_on_control(), Some(after));
        });
        assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
        assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
    }
    let retired = effect.retire_on_control().unwrap();
    assert!(retired.failure.is_none());
    assert_eq!(retired.native.terminal.unwrap().blocks, 0);
    assert_eq!(effect.constructor_storage_on_control(), Some(after));
}

struct Fixture {
    editor: Editor,
    media: BTreeMap<hex_project::AssetId, hex_engine::Audio>,
    effect: NativeEffect,
    device: hex_project::DeviceId,
    samples: Vec<[f32; 2]>,
}
fn fixture(location: usize, bypassed: bool) -> Fixture {
    fixture_with_authored_parameter(location, bypassed, None)
}

fn fixture_with_authored_parameter(
    location: usize,
    bypassed: bool,
    parameter: Option<u32>,
) -> Fixture {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "source".into(),
            path: "memory.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 32,
            playback_frames: 32,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("original audio import")
    };
    let mut bus = None;
    let owner = match location {
        0 => ProcessorChainOwner::Track(track.value()),
        1 => {
            let Created::ReturnTrack(id) = editor
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
                    return_track: id,
                    gain: 1.0,
                })
                .unwrap();
            bus = Some(id);
            ProcessorChainOwner::Return(id.value())
        }
        _ => ProcessorChainOwner::Main,
    };
    let identity = ProcessorInstanceIdentity::new(owner, editor.project().next_identity()).unwrap();
    let source = original_instance_for_binding(Binding {
        root: 1,
        instance: identity.device_id(),
        graph: 3,
        epoch: 4,
    });
    let selected = parameter.map(|id| [id]);
    let class = source.configuration().identity.class;
    let effect = NativeEffect::prepare_retained_with_authored_selection_on_control(
        identity,
        source,
        usize::MAX,
        selected.as_ref().map(|ids| (class, ids.as_slice())),
    )
    .unwrap();
    let state = effect.configuration().clone();
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
    let mut samples = vec![[0.0; 2]; 32];
    samples[0] = [0.25, -0.5];
    samples[31] = [-0.125, 0.375];
    let media = BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48_000, samples.clone()).unwrap(),
    )]);
    Fixture {
        editor,
        media,
        effect,
        device,
        samples,
    }
}
fn prepare(f: &Fixture) -> Arc<hex_engine::PreparedRevision> {
    let definition = NativeEffectDefinition::new(std::slice::from_ref(&f.effect)).unwrap();
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let effects = NativeDeviceRegistry::new(&definitions).unwrap();
    Arc::new(
        hex_engine::PreparedRevision::prepare_with_effect_registry(
            f.editor.project(),
            &f.media,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &effects,
        )
        .unwrap(),
    )
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
#[cfg(feature = "allocation-probe")]
fn actual_native_graph_track_return_main_and_aligned_bypass_have_exact_pcm_without_callback_allocations()
 {
    for location in 0..3 {
        for bypassed in [false, true] {
            let f = fixture(location, bypassed);
            let revision = prepare(&f);
            let mut playback =
                hex_engine::playback::Playback::try_from_prepared_with_session(revision).unwrap();
            playback.apply(hex_engine::PlaybackCommand::Play);
            let (mut workers, mut callback) =
                NativeEffectCallbacks::from_effects_on_control(vec![f.effect]).unwrap();
            let mut worker = workers.pop().unwrap();
            let mut output = vec![[9.0; 2]; 64];
            let mut rendered = 0;
            for chunk in output.chunks_mut(5) {
                let mut accepted = false;
                let measured = allocation_counter::measure(|| {
                    callback.begin_host_callback(chunk.len()).unwrap();
                    playback
                        .render_with_foreign_processors(chunk, None, None, None, &mut callback)
                        .unwrap();
                    // The final f32 host conversion is identity and checked
                    // here, outside the engine's success/transaction boundary.
                    accepted = chunk.iter().flatten().all(|sample| sample.is_finite());
                    callback.finish_host_callback(accepted).unwrap();
                });
                assert!(accepted);
                assert_eq!(
                    (
                        measured.count_total,
                        measured.bytes_total,
                        measured.count_current,
                        measured.bytes_current
                    ),
                    (0, 0, 0, 0)
                );
                rendered += chunk.len();
                let deadline = Instant::now() + Duration::from_secs(3);
                while worker.published_blocks_on_control() < (rendered / 8) as u64 {
                    assert!(
                        Instant::now() < deadline,
                        "actual native worker publication"
                    );
                    std::thread::yield_now();
                }
            }
            let mut expected = vec![[0.0; 2]; 64];
            for (i, sample) in f.samples.iter().enumerate() {
                let gain = (if bypassed {
                    1.0
                } else {
                    0.5 + (i / 8) as f32 * 0.125
                }) + if location == 1 { 1.0 } else { 0.0 };
                expected[i + 16] = [sample[0] * gain, sample[1] * gain];
            }
            assert_eq!(
                output, expected,
                "actual SDK owner {location}, bypass {bypassed}"
            );
            let stopped = worker.quiesce_on_control().unwrap();
            assert_eq!(stopped.completed_blocks(), 8);
            drop(callback);
            drop(worker);
            let terminal = stopped.retire_on_control().unwrap().terminal.unwrap();
            assert_eq!((terminal.blocks, terminal.frames), (8, 64));
        }
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_source_factory_refuses_wrong_instance_and_host_rejection_never_dispatches() {
    let mut f = fixture(0, false);
    let revision = prepare(&f);
    let state = f.effect.configuration().clone();
    let track = f.editor.project().tracks()[0].id();
    f.editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap();
    let definition = NativeEffectDefinition::new(std::slice::from_ref(&f.effect)).unwrap();
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let effects = NativeDeviceRegistry::new(&definitions).unwrap();
    assert!(
        hex_engine::PreparedRevision::prepare_with_effect_registry(
            f.editor.project(),
            &f.media,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &effects
        )
        .is_err(),
        "equal state is not another source instance"
    );
    drop(definition);
    let mut playback =
        hex_engine::playback::Playback::try_from_prepared_with_session(revision).unwrap();
    playback.apply(hex_engine::PlaybackCommand::Play);
    let mut unavailable = [[9.0; 2]; 8];
    assert!(playback.render(&mut unavailable).is_err());
    assert_eq!(unavailable, [[0.0; 2]; 8]);
    assert_eq!(playback.position(), 0);
    let (mut worker, mut callback) = f.effect.into_parts();
    let mut output = [[9.0; 2]; 8];
    callback.begin_host_callback(output.len()).unwrap();
    playback
        .render_with_foreign_processors(&mut output, None, None, None, &mut callback)
        .unwrap();
    assert_eq!(
        worker.published_blocks_on_control(),
        0,
        "render success did not dispatch"
    );
    assert!(callback.finish_host_callback(false).is_err());
    let stopped = worker.quiesce_on_control().unwrap();
    assert_eq!(
        stopped.completed_blocks(),
        0,
        "final host refusal publishes no input"
    );
    drop(callback);
    drop(worker);
    assert!(stopped.retire_on_control().unwrap().status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_bypass_handoff_retains_untrimmed_dry_history() {
    for location in 0..3 {
        let mut f = fixture(location, false);
        let state = f
            .effect
            .configuration()
            .clone()
            .with_input_gain_millidb(6000)
            .unwrap();
        f.editor
            .execute(Command::ReplaceDeviceState {
                device: f.device,
                state,
            })
            .unwrap();
        let mut before =
            hex_engine::playback::Playback::try_from_prepared_with_session(prepare(&f)).unwrap();
        before.apply(hex_engine::PlaybackCommand::Play);
        f.editor
            .execute(Command::SetDeviceBypass {
                device: f.device,
                bypassed: true,
            })
            .unwrap();
        let mut after =
            hex_engine::playback::Playback::try_from_prepared_with_session(prepare(&f)).unwrap();
        f.editor.execute(Command::RemoveDevice(f.device)).unwrap();
        let mut removed =
            hex_engine::playback::Playback::try_from_prepared_with_session(prepare(&f)).unwrap();
        let (mut worker, mut callback) = f.effect.into_parts();
        for block in 0..2 {
            let mut output = [[9.0; 2]; 8];
            callback.begin_host_callback(8).unwrap();
            before
                .render_with_foreign_processors(&mut output, None, None, None, &mut callback)
                .unwrap();
            callback.finish_host_callback(true).unwrap();
            assert_eq!(output, [[0.0; 2]; 8]);
            let deadline = Instant::now() + Duration::from_secs(3);
            while worker.published_blocks_on_control() < block + 1 {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
        if location < 2 {
            assert!(
                removed.try_inherit_runtime_from(&mut before).is_err(),
                "positive-latency Track/Return removal still requires retained-route admission"
            );
        }
        assert_eq!(before.position(), 16);
        let measured =
            allocation_counter::measure(|| after.try_inherit_runtime_from(&mut before).unwrap());
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let mut output = [[9.0; 2]; 8];
        callback.begin_host_callback(8).unwrap();
        after
            .render_with_foreign_processors(&mut output, None, None, None, &mut callback)
            .unwrap();
        callback.finish_host_callback(true).unwrap();
        let input_gain = hex_lattice_contract::HostGain::from_millidb(6000)
            .unwrap()
            .linear();
        for (i, actual) in output.iter().enumerate() {
            let wet_mix = 1.0 - (i + 1) as f32 / 240.0;
            for channel in 0..2 {
                let dry = f.samples[i][channel];
                let wet = dry * input_gain * 0.5; // SDK fixture's first processed quantum.
                let expected =
                    dry * (1.0 - wet_mix) + wet * wet_mix + if location == 1 { dry } else { 0.0 };
                assert!(
                    (actual[channel] - expected).abs() < 1e-5,
                    "aligned bypass frame {i} channel {channel}: {actual:?} expected {expected}"
                );
            }
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while worker.published_blocks_on_control() < 3 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let stopped = worker.quiesce_on_control().unwrap();
        drop(callback);
        drop(worker);
        assert!(stopped.retire_on_control().unwrap().status.success());
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn equal_state_and_device_id_cannot_select_another_actual_native_epoch() {
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 2).unwrap();
    let first = NativeEffect::prepare_on_control(
        identity,
        original_instance_for_binding(Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        }),
    )
    .unwrap();
    let second = NativeEffect::prepare_on_control(
        identity,
        original_instance_for_binding(Binding {
            root: 1,
            instance: 2,
            graph: 4,
            epoch: 5,
        }),
    )
    .unwrap();
    assert!(first.configuration().processing_eq(second.configuration()));
    let (second_worker, mut callback) = second.into_parts();
    assert!(callback.matches_worker(&second_worker));
    callback.begin_host_callback(8).unwrap();
    let node = ForeignProcessorNode {
        owner: identity.owner(),
        device_id: identity.device_id(),
        device_type: hex_plugin_state::TYPE_ID,
        sample_rate: 48_000,
        configuration: first.configuration(),
        prepared: first.prepared(),
        automation: None,
        bypassed: false,
        input_gain: 1.0,
        output_gain: 1.0,
    };
    let mut refused = Ok(());
    let measured = allocation_counter::measure(|| {
        refused = callback.process_segment(node, 0, &mut [[1.0; 2]; 8], None);
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        refused,
        Err(DeviceContractError::ForeignProcessingUnavailable),
        "old/new native leases with equal saved state are distinct source epochs"
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_removed_main_effect_keeps_its_original_native_pipeline_tail() {
    let mut f = fixture(2, false);
    let mut before =
        hex_engine::playback::Playback::try_from_prepared_with_session(prepare(&f)).unwrap();
    before.apply(hex_engine::PlaybackCommand::Play);
    f.editor.execute(Command::RemoveDevice(f.device)).unwrap();
    let mut after =
        hex_engine::playback::Playback::try_from_prepared_with_session(prepare(&f)).unwrap();
    let (mut worker, mut callback) = f.effect.into_parts();
    for block in 0..2 {
        callback.begin_host_callback(8).unwrap();
        before
            .render_with_foreign_processors(&mut [[0.0; 2]; 8], None, None, None, &mut callback)
            .unwrap();
        callback.finish_host_callback(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while worker.published_blocks_on_control() < block + 1 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    let measured =
        allocation_counter::measure(|| after.try_inherit_runtime_from(&mut before).unwrap());
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let mut output = [[9.0; 2]; 8];
    callback.begin_host_callback(8).unwrap();
    after
        .render_with_foreign_processors(&mut output, None, None, None, &mut callback)
        .unwrap();
    callback.finish_host_callback(true).unwrap();
    assert_eq!(
        output[0],
        [f.samples[0][0] * 0.5, f.samples[0][1] * 0.5],
        "actual previously accepted input is not dropped or recreated at removal"
    );
    assert_eq!(&output[1..], &[[0.0; 2]; 7]);
    let deadline = Instant::now() + Duration::from_secs(3);
    while worker.published_blocks_on_control() < 3 {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let stopped = worker.quiesce_on_control().unwrap();
    assert_eq!(stopped.completed_blocks(), 3);
    drop(callback);
    drop(worker);
    let terminal = stopped.retire_on_control().unwrap().terminal.unwrap();
    assert_eq!((terminal.blocks, terminal.frames), (3, 24));
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
#[cfg(feature = "allocation-probe")]
fn actual_multiple_native_nodes_route_exact_epochs_and_accept_only_final_host_output() {
    let mut f = fixture(0, false);
    let identity = ProcessorInstanceIdentity::new(
        ProcessorChainOwner::Main,
        f.editor.project().next_identity(),
    )
    .unwrap();
    let main = NativeEffect::prepare_on_control(
        identity,
        original_instance_for_binding(Binding {
            root: 1,
            instance: identity.device_id(),
            graph: 3,
            epoch: 4,
        }),
    )
    .unwrap();
    let Created::Device(main_device) = f
        .editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: main.configuration().clone(),
        })
        .unwrap()
    else {
        panic!("main device")
    };
    assert_eq!(main_device.value(), identity.device_id());
    let mut sources = vec![main, f.effect]; // Deliberately unrelated to graph order.
    let definition = NativeEffectDefinition::new(&sources).unwrap();
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare_with_effect_registry(
            f.editor.project(),
            &f.media,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &registry,
        )
        .unwrap(),
    );
    drop(definition);
    // An actual other epoch with identical device/class/state coexists in the
    // carrier, as it will during source replacement/retained-tail return. Its
    // presence must neither hijack the graph nor fabricate zero-input work.
    let unused_binding = Binding {
        graph: 4,
        epoch: 5,
        ..sources[1].binding()
    };
    sources.push(
        NativeEffect::prepare_on_control(
            sources[1].identity(),
            original_instance_for_binding(unused_binding),
        )
        .unwrap(),
    );
    sources.reverse();
    let (mut workers, mut callbacks) =
        NativeEffectCallbacks::from_effects_on_control(sources).unwrap();
    let mut playback =
        hex_engine::playback::Playback::try_from_prepared_with_session(revision).unwrap();
    let mut stopped_output = [[9.0; 2]; 8];
    callbacks.begin_host_callback(8).unwrap();
    playback
        .render_with_foreign_processors(&mut stopped_output, None, None, None, &mut callbacks)
        .unwrap();
    callbacks.finish_host_callback(true).unwrap();
    assert_eq!(stopped_output, [[0.0; 2]; 8]);
    assert!(workers.iter().all(|w| w.published_blocks_on_control() == 0));
    playback.apply(hex_engine::PlaybackCommand::Play);
    let mut output = vec![[9.0; 2]; 96];
    let mut rendered = 0;
    for chunk in output.chunks_mut(5) {
        let measured = allocation_counter::measure(|| {
            callbacks.begin_host_callback(chunk.len()).unwrap();
            playback
                .render_with_foreign_processors(chunk, None, None, None, &mut callbacks)
                .unwrap();
            assert!(chunk.iter().flatten().all(|x| x.is_finite()));
            callbacks.finish_host_callback(true).unwrap();
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        rendered += chunk.len();
        let deadline = Instant::now() + Duration::from_secs(3);
        for worker in &workers {
            let blocks = if worker.binding() == unused_binding {
                0
            } else {
                (rendered / 8) as u64
            };
            while worker.published_blocks_on_control() < blocks {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            assert_eq!(worker.published_blocks_on_control(), blocks);
        }
    }
    let mut expected = vec![[0.0; 2]; 96];
    for (i, sample) in f.samples.iter().enumerate() {
        let track_gain = 0.5 + (i / 8) as f32 * 0.125;
        let main_gain = 0.5 + ((i + 16) / 8) as f32 * 0.125;
        expected[i + 32] = [
            sample[0] * track_gain * main_gain,
            sample[1] * track_gain * main_gain,
        ];
    }
    assert_eq!(
        output, expected,
        "two real SDK nodes in the canonical graph"
    );
    // Complete graph render still cannot dispatch a single node before final
    // host acceptance. Reject every staged endpoint, including the inactive one.
    callbacks.begin_host_callback(8).unwrap();
    playback
        .render_with_foreign_processors(&mut [[0.0; 2]; 8], None, None, None, &mut callbacks)
        .unwrap();
    assert!(callbacks.finish_host_callback(false).is_err());
    assert!(callbacks.begin_host_callback(8).is_err());
    let mut stopped = Vec::new();
    for worker in &mut workers {
        let source = worker.quiesce_on_control().unwrap();
        assert_eq!(
            source.completed_blocks(),
            if worker.binding() == unused_binding {
                0
            } else {
                12
            }
        );
        stopped.push(source);
    }
    drop(callbacks);
    drop(workers);
    for source in stopped {
        assert!(source.retire_on_control().unwrap().status.success());
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
#[cfg(feature = "allocation-probe")]
fn registered_sampler_and_real_native_effect_share_the_resident_compiler_and_original_resources() {
    // Match the existing bounded production preparation-worker stack.
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(sampler_with_native_effect_on_control)
        .unwrap()
        .join()
        .unwrap();
}

#[cfg(feature = "allocation-probe")]
fn sampler_with_native_effect_on_control() {
    use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
    use hex_project::BeatTime;
    use std::sync::atomic::AtomicBool;
    let mut f = fixture(2, false);
    let track = f.editor.project().tracks()[0].id();
    let Created::Asset(asset) = f
        .editor
        .execute(Command::ImportDeviceAsset {
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("original device asset")
    };
    let mut sampler = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
    sampler.mode = hex_devices::sampler::state::AuthoredMode::OneShot {
        gate: hex_devices::sampler::GateMode::Trigger,
    };
    sampler.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.0,
    };
    f.editor
        .execute(Command::InsertInstrument {
            track,
            state: sampler
                .to_native()
                .unwrap()
                .with_asset_references(vec![DeviceAssetReference {
                    slot: DeviceAssetSlot::new(1).unwrap(),
                    asset: DeviceAssetKey::new(asset.value()).unwrap(),
                }])
                .unwrap(),
        })
        .unwrap();
    let Created::Clip(clip) = f
        .editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("hybrid MIDI clip")
    };
    f.editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let cancel = AtomicBool::new(false);
    let limits = hex_engine::AudioStorageLedgerReservation {
        data_rows: 64,
        handle_rows: 64,
        root_rows: 16,
        max_retained_bytes: 20_000_000,
        max_construction_peak_bytes: 20_000_000,
    };
    let mut store = ResidentDeviceAssets::default();
    store
        .insert_on_control(
            (
                asset,
                hex_engine::OwnedAudioSource::try_resident(
                    48_000,
                    vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0; 2]],
                )
                .ok()
                .unwrap(),
            ),
            0,
            limits,
            4,
            &cancel,
            || Ok::<_, hex_engine::native_device_resources::ResourcePreparationError>(()),
        )
        .unwrap();
    let resources = PreparedDeviceResources::prepare_on_control(
        &store,
        f.editor.project(),
        20_000_000,
        &cancel,
    )
    .unwrap();
    let definition = NativeEffectDefinition::new(std::slice::from_ref(&f.effect)).unwrap();
    let builtins = hex_devices::native_registry().unwrap();
    let mut definitions = builtins.definitions().to_vec();
    definitions.push(&definition);
    let effects = NativeDeviceRegistry::new(&definitions).unwrap();
    let transforms = hex_engine::audio_transform::AudioTransformPreparation::default();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare_with_device_resources_and_effect_registry(
            f.editor.project(),
            &f.media,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &transforms,
            &resources,
            resources
                .compiler_scan_work_on_control(f.editor.project())
                .unwrap(),
            &cancel,
            &effects,
        )
        .unwrap(),
    );
    assert!(
        hex_engine::playback::Playback::try_from_prepared_with_session(Arc::clone(&revision))
            .is_err(),
        "foreign facts cannot replace the original registered sampler resource set"
    );
    let mut playback = hex_engine::playback::Playback::try_from_prepared_with_device_resources(
        revision, &resources,
    )
    .unwrap();
    // Compile an independent dry graph against the very same original source
    // store; no PCM is reconstructed from saved state or effect facts.
    f.editor.execute(Command::RemoveDevice(f.device)).unwrap();
    let dry_resources = PreparedDeviceResources::prepare_on_control(
        &store,
        f.editor.project(),
        20_000_000,
        &cancel,
    )
    .unwrap();
    let dry_revision = Arc::new(
        hex_engine::PreparedRevision::prepare_with_device_resources(
            f.editor.project(),
            &f.media,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &transforms,
            &dry_resources,
            dry_resources
                .compiler_scan_work_on_control(f.editor.project())
                .unwrap(),
            &cancel,
        )
        .unwrap(),
    );
    let mut dry = hex_engine::playback::Playback::try_from_prepared_with_device_resources(
        dry_revision,
        &dry_resources,
    )
    .unwrap();
    dry.apply(hex_engine::PlaybackCommand::Play);
    let mut reference = [[0.0; 2]; 48];
    dry.render(&mut reference).unwrap();
    assert!(
        reference[..4] != f.samples[..4],
        "the registered sampler actually contributes alongside the audio clip"
    );
    playback.apply(hex_engine::PlaybackCommand::Play);
    let (mut worker, mut callbacks) = f.effect.into_parts();
    let mut output = [[0.0; 2]; 64];
    for (index, chunk) in output.chunks_mut(8).enumerate() {
        let measured = allocation_counter::measure(|| {
            callbacks.begin_host_callback(8).unwrap();
            playback
                .render_with_foreign_processors(chunk, None, None, None, &mut callbacks)
                .unwrap();
            callbacks.finish_host_callback(true).unwrap();
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while worker.published_blocks_on_control() < index as u64 + 1 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    assert_eq!(output[..16], [[0.0; 2]; 16]);
    for (index, frame) in reference.iter().enumerate() {
        let gain = 0.5 + (index / 8) as f32 * 0.125;
        assert_eq!(output[index + 16], [frame[0] * gain, frame[1] * gain]);
    }
    let stopped = worker.quiesce_on_control().unwrap();
    drop(callbacks);
    drop(worker);
    assert!(stopped.retire_on_control().unwrap().status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
#[cfg(feature = "allocation-probe")]
fn actual_sdk_closed_claimed_renderer_track_return_main_has_exact_pcm_and_disposal() {
    for location in 0..3 {
        for bypassed in [false, true] {
            let f = fixture(location, bypassed);
            let mut owner = {
                let definition =
                    NativeEffectDefinition::new(std::slice::from_ref(&f.effect)).unwrap();
                let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
                let registry = NativeDeviceRegistry::new(&definitions).unwrap();
                let sources = f
                    .media
                    .iter()
                    .map(|(id, audio)| {
                        (
                            *id,
                            hex_engine::OwnedAudioSource::try_resident(
                                audio.sample_rate(),
                                audio.frames().to_vec(),
                            )
                            .ok()
                            .unwrap(),
                        )
                    })
                    .collect();
                let bank = hex_engine::OwnedAudioInputs::try_new(
                    sources,
                    hex_engine::AudioStorageReservation {
                        allocation_items: 64,
                        handle_obligations: 256,
                        max_prepared_data_bytes: usize::MAX,
                        max_construction_data_bytes: usize::MAX,
                        caller_retained_bytes: 0,
                        caller_construction_bytes: 0,
                    },
                )
                .ok()
                .unwrap();
                let mut custody =
                    hex_engine::AudioPreparationCustody::try_new_on_control().unwrap();
                let token = custody
                    .begin_input_on_control(
                        hex_engine::AudioArrangementInput::new(f.editor.project(), 11, bank)
                            .with_audio_prepared_revision()
                            .with_effect_registry(&registry),
                    )
                    .ok()
                    .unwrap();
                hex_engine::AudioCandidateOwner::try_new_on_control(custody, token)
                    .ok()
                    .expect("closed actual SDK graph")
            };
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
            assert!(owner.requires_effect_graph_custody_on_control().unwrap());
            let (mut runtime, anchor) = owner.split_constructor_on_control().ok().unwrap();
            assert!(anchor.matches_current(&runtime));
            runtime
                .apply_action(hex_engine::PlaybackAction::Transport(
                    hex_engine::PlaybackCommand::Play,
                ))
                .unwrap();
            let (mut workers, mut callback) =
                NativeEffectCallbacks::from_effects_on_control(vec![f.effect]).unwrap();
            let mut worker = workers.pop().unwrap();
            let mut output = [[9.0; 2]; 64];
            let mut rendered = 0;
            for chunk in output.chunks_mut(5) {
                let measured = allocation_counter::measure(|| {
                    callback.begin_host_callback(chunk.len()).unwrap();
                    runtime
                        .render_with_foreign_processors(chunk, None, None, None, &mut callback)
                        .unwrap();
                    let accepted = chunk.iter().flatten().all(|sample| sample.is_finite());
                    callback.finish_host_callback(accepted).unwrap();
                });
                assert_eq!(
                    (
                        measured.count_total,
                        measured.bytes_total,
                        measured.count_current,
                        measured.bytes_current
                    ),
                    (0, 0, 0, 0)
                );
                rendered += chunk.len();
                let deadline = Instant::now() + Duration::from_secs(3);
                while worker.published_blocks_on_control() < (rendered / 8) as u64 {
                    assert!(Instant::now() < deadline, "original SDK reply publication");
                    std::thread::yield_now();
                }
            }
            let mut expected = [[0.0; 2]; 64];
            for (i, sample) in f.samples.iter().enumerate() {
                let gain = (if bypassed {
                    1.0
                } else {
                    0.5 + (i / 8) as f32 * 0.125
                }) + if location == 1 { 1.0 } else { 0.0 };
                expected[i + 16] = [sample[0] * gain, sample[1] * gain];
            }
            assert_eq!(
                output, expected,
                "closed claimed SDK owner {location}, bypass {bypassed}"
            );
            let proof = runtime.dispose_on_control().unwrap();
            let mut anchors = [Some(anchor)];
            proof.record_anchor_cells_on_control(&mut anchors).unwrap();
            let anchor = anchors[0].as_mut().unwrap();
            anchor.finish_disposal_on_control().unwrap();
            assert!(anchor.recover_inputs_on_control().is_some());
            let stopped = worker.quiesce_on_control().unwrap();
            assert_eq!(stopped.completed_blocks(), 8);
            drop(callback);
            drop(worker);
            let terminal = stopped.retire_on_control().unwrap().terminal.unwrap();
            assert_eq!((terminal.blocks, terminal.frames), (8, 64));
        }
    }
}

#[cfg(feature = "allocation-probe")]
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_tail_drain_waits_for_final_acceptance_and_preserves_last_bridge_samples() {
    for refuse_at_horizon in [false, true] {
        let binding = Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let instance = original_instance_for_binding(binding);
        assert_eq!(instance.ready().tail_frames, 0);
        let effect = NativeEffect::prepare_on_control(
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 2).unwrap(),
            instance,
        )
        .unwrap();
        let config = effect.callback.configuration.clone();
        let node = ForeignProcessorNode {
            owner: config.identity.owner(),
            device_id: config.identity.device_id(),
            device_type: config.state.type_id(),
            sample_rate: config.sample_rate,
            configuration: &config.state,
            prepared: config.prepared,
            automation: None,
            bypassed: false,
            input_gain: 1.0,
            output_gain: 1.0,
        };
        let (mut worker, mut callback) = effect.into_parts();
        assert!(!callback.reported_tail_is_drained());
        for block in 0..3 {
            let mut frames = if block == 0 {
                [[0.5, -0.25]; 8]
            } else {
                [[0.0; 2]; 8]
            };
            let accepted = !(refuse_at_horizon && block == 2);
            let measured = allocation_counter::measure(|| {
                callback.begin_host_callback(8).unwrap();
                callback
                    .process_segment(node, 0, &mut frames[..3], None)
                    .unwrap();
                callback
                    .process_segment(node, 3, &mut frames[3..], None)
                    .unwrap();
                assert!(
                    !callback.reported_tail_is_drained(),
                    "staged zeros are not accepted output"
                );
                assert_eq!(callback.finish_host_callback(accepted).is_ok(), accepted);
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            if block == 2 {
                assert_eq!(
                    frames,
                    [[0.25, -0.125]; 8],
                    "last original nonzero samples cross the bridge at the drain horizon"
                );
            }
            assert_eq!(callback.reported_tail_is_drained(), block == 2 && accepted);
            if accepted {
                let deadline = Instant::now() + Duration::from_secs(3);
                while worker.published_blocks_on_control() < block + 1 {
                    assert!(Instant::now() < deadline);
                    std::thread::yield_now();
                }
            }
        }
        if !refuse_at_horizon {
            let mut renewed = [[0.5; 2]; 8];
            let measured = allocation_counter::measure(|| {
                callback.begin_host_callback(8).unwrap();
                callback
                    .process_segment(node, 0, &mut renewed, None)
                    .unwrap();
                assert!(
                    !callback.reported_tail_is_drained(),
                    "new staged input invalidates an old drained view"
                );
                assert!(callback.finish_host_callback(false).is_err());
                assert!(!callback.reported_tail_is_drained());
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        let stopped = worker.quiesce_on_control().unwrap();
        assert_eq!(
            stopped.completed_blocks(),
            if refuse_at_horizon { 2 } else { 3 }
        );
        drop(callback);
        drop(worker);
        assert!(stopped.retire_on_control().unwrap().status.success());
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
#[cfg(feature = "allocation-probe")]
fn actual_arrangement_instrument_renders_authored_notes_at_compensated_offsets() {
    const NAME: &str = "plugin_effects::tests::actual_arrangement_instrument_renders_authored_notes_at_compensated_offsets";
    const CHILD: &str = "HEX_ARRANGEMENT_INSTRUMENT_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        for variant in ["115", "120"] {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--ignored"])
                .env(CHILD, "1")
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", variant)
                .output()
                .unwrap();
            assert!(
                result.status.success()
                    && String::from_utf8_lossy(&result.stdout).contains("1 passed"),
                "variant {variant}\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr),
            );
        }
        return;
    }

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "synth".into(),
        })
        .unwrap()
    else {
        panic!("midi track");
    };
    let identity = ProcessorInstanceIdentity::new(
        ProcessorChainOwner::Track(track.value()),
        editor.project().next_identity(),
    )
    .unwrap();
    let source = crate::plugin_worker::native_tests::original_instance_with_inputs(
        Binding {
            root: 1,
            instance: identity.device_id(),
            graph: 3,
            epoch: 4,
        },
        0,
    );
    let effect = NativeEffect::prepare_on_control(identity, source).unwrap();
    editor
        .execute(Command::InsertInstrument {
            track,
            state: effect.configuration().clone(),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_ratio(1, 1).unwrap(),
        })
        .unwrap()
    else {
        panic!("midi clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::from_ratio(1, 24_000).unwrap(),
            duration: hex_project::BeatTime::from_ratio(1, 12_000).unwrap(),
            pitch: 60,
            channel: 2,
            velocity: 127,
        })
        .unwrap();
    let revision = {
        let definition = NativeEffectDefinition::new(std::slice::from_ref(&effect)).unwrap();
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        hex_engine::PreparedRevision::prepare_with_effect_registry(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
            &registry,
        )
        .unwrap()
    };
    let latency = revision.arrangement().main_latency_frames() as usize;
    assert_eq!(latency, effect.prepared().total_latency_frames() as usize);
    let mut runtime = hex_engine::SessionPlayback::new(Arc::new(revision)).unwrap();
    let (mut workers, mut endpoints) =
        NativeEffectCallbacks::from_effects_on_control(vec![effect]).unwrap();
    runtime
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    let mut output = [[0.0; 2]; 32];
    for (index, block) in output.chunks_exact_mut(8).enumerate() {
        let measured = allocation_counter::measure(|| {
            endpoints.begin_host_callback(block.len()).unwrap();
            runtime
                .render_with_foreign_processors(block, None, None, None, &mut endpoints)
                .unwrap();
            endpoints.finish_host_callback(true).unwrap();
        });
        assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
        assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
        let deadline = Instant::now() + Duration::from_secs(1);
        while workers[0].published_blocks_on_control() <= index as u64 {
            assert!(Instant::now() < deadline, "native pcm completion");
            std::thread::yield_now();
        }
    }
    let mut expected = [[0.0; 2]; 32];
    expected[latency + 1..latency + 3].fill([0.5; 2]);
    assert_eq!(output, expected);
    let retired = workers[0].retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
    assert_eq!(retired.native.terminal.unwrap().blocks, 4);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
#[cfg(feature = "allocation-probe")]
fn actual_arrangement_renderer_clock_drives_the_signed_authored_effect() {
    use hex_engine::{PlaybackAction, PlaybackCommand};
    use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime};

    const NAME: &str = "plugin_effects::tests::actual_arrangement_renderer_clock_drives_the_signed_authored_effect";
    const CHILD: &str = "HEX_ARRANGEMENT_AUTHORED_EFFECT_TEST_CHILD";
    let Some(scenario) = std::env::var_os(CHILD) else {
        for scenario in ["pause", "loop-seek", "loop-live"] {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--ignored", "--nocapture"])
                .env(CHILD, scenario)
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", "91")
                .output()
                .unwrap();
            assert!(
                result.status.success()
                    && String::from_utf8_lossy(&result.stdout).contains("1 passed"),
                "{scenario}: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        return;
    };
    let loop_live = scenario == "loop-live";
    let loop_seek = scenario == "loop-seek" || loop_live;

    let mut fixture = fixture_with_authored_parameter(2, false, Some(7));
    fixture.samples[8] = [0.125, -0.25];
    *fixture.media.values_mut().next().unwrap() =
        hex_engine::Audio::new(48_000, fixture.samples.clone()).unwrap();
    fixture
        .editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device: fixture.device,
            target: hex_project::DeviceAutomationTarget::Vst3 { parameter: 7 },
            points: [
                (BeatTime::ZERO, 0.25, AutomationInterpolation::Linear),
                (
                    BeatTime::from_ratio(1, 1500).unwrap(),
                    0.5,
                    AutomationInterpolation::Step,
                ),
                (
                    BeatTime::from_ratio(1, 800).unwrap(),
                    0.75,
                    AutomationInterpolation::Step,
                ),
            ]
            .map(|(beat, value, outgoing)| AutomationPoint {
                beat,
                value: AutomationValue::new(value).unwrap(),
                outgoing,
            })
            .to_vec(),
        })
        .unwrap();
    if loop_seek {
        fixture
            .editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 512,
                enabled: true,
            })
            .unwrap();
    }
    let prepare_candidate = |project: &Project, effect: &NativeEffect, generation: u64| {
        let definition = NativeEffectDefinition::new(std::slice::from_ref(effect)).unwrap();
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let effects = hex_engine::AudioCandidateEffects::new(&registry);
        let (&asset, media) = fixture.media.first_key_value().unwrap();
        let quote = effects.quote(project, 1).unwrap();
        let limits = quote
            .storage_reservation(std::mem::size_of_val(media.frames()))
            .unwrap();
        let bank = hex_engine::OwnedAudioInputs::try_new(
            vec![(
                asset,
                hex_engine::OwnedAudioSource::try_resident(48_000, media.frames().to_vec())
                    .ok()
                    .unwrap(),
            )],
            limits,
        )
        .ok()
        .unwrap();
        let mut custody = hex_engine::AudioPreparationCustody::try_new_on_control().unwrap();
        let token = custody
            .begin_input_on_control(
                hex_engine::AudioArrangementInput::new(project, generation, bank)
                    .with_audio_prepared_revision()
                    .with_effect_registry(&registry),
            )
            .ok()
            .unwrap();
        let mut owner = hex_engine::AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        owner
            .seal_constructor_on_control(hex_engine::AudioStorageLedgerReservation {
                data_rows: quote.data_rows,
                handle_rows: quote.handle_rows,
                root_rows: 1,
                max_retained_bytes: limits.max_prepared_data_bytes,
                max_construction_peak_bytes: limits.max_construction_data_bytes,
            })
            .unwrap();
        owner.split_constructor_on_control().ok().unwrap()
    };
    let mut previous =
        loop_live.then(|| prepare_candidate(fixture.editor.project(), &fixture.effect, 10));
    if loop_seek {
        fixture
            .editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 320,
                enabled: true,
            })
            .unwrap();
    }
    let mut effects = vec![fixture.effect];
    if loop_live {
        let original = &effects[0];
        let mut binding = original.binding();
        binding.epoch += 1;
        let instance = original_instance_for_binding(binding);
        let class = instance.configuration().identity.class;
        effects.push(
            NativeEffect::prepare_retained_with_authored_selection_on_control(
                original.identity(),
                instance,
                usize::MAX,
                Some((class, &[7])),
            )
            .unwrap(),
        );
    }
    let path = std::env::temp_dir().join(format!(
        "hex-authored-arrangement-{}.hex",
        std::process::id()
    ));
    hex_storage::save_new(&path, fixture.editor.project()).unwrap();
    let reopened = hex_storage::load(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    let (mut runtime, anchor) = prepare_candidate(&reopened, effects.last().unwrap(), 11);
    let (mut workers, mut endpoints) =
        NativeEffectCallbacks::from_effects_on_control(effects).unwrap();
    if loop_seek && !loop_live {
        runtime
            .apply_action(PlaybackAction::Transport(PlaybackCommand::Seek(304)))
            .unwrap();
    }
    if !loop_live {
        runtime
            .apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
            .unwrap();
    }
    let mut retired_graph = None;
    let mut retired_anchor = None;
    if let Some((mut old, old_anchor)) = previous.take() {
        old.apply_action(PlaybackAction::Transport(PlaybackCommand::Seek(304)))
            .unwrap();
        old.apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
            .unwrap();
        let mut before_edit = [[0.0; 2]; 5];
        endpoints.begin_host_callback(before_edit.len()).unwrap();
        old.render_with_foreign_processors(&mut before_edit, None, None, None, &mut endpoints)
            .unwrap();
        endpoints.finish_host_callback(true).unwrap();
        assert_eq!(before_edit, [[0.0; 2]; 5]);
        let mut transfer = None;
        runtime
            .try_inherit_and_retire_in_place(&mut Some(old), &mut retired_graph, &mut transfer)
            .unwrap();
        assert!(transfer.is_some());
        retired_anchor = Some(old_anchor);
    }
    let mut output = [[0.0; 2]; 64];
    let mut rendered = 0;
    for frames in output.chunks_mut(5) {
        if !loop_seek && rendered == 5 {
            runtime
                .apply_action(PlaybackAction::Transport(PlaybackCommand::Pause))
                .unwrap();
            let mut paused = [[9.0; 2]; 3];
            endpoints.begin_host_callback(paused.len()).unwrap();
            runtime
                .render_with_foreign_processors(&mut paused, None, None, None, &mut endpoints)
                .unwrap();
            endpoints.finish_host_callback(true).unwrap();
            assert_eq!(paused, [[0.0; 2]; 3]);
            runtime
                .apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
                .unwrap();
        }
        let measured = allocation_counter::measure(|| {
            endpoints.begin_host_callback(frames.len()).unwrap();
            runtime
                .render_with_foreign_processors(frames, None, None, None, &mut endpoints)
                .unwrap();
            endpoints.finish_host_callback(true).unwrap();
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        rendered += frames.len();
        endpoints
            .wait_offline_on_control(&workers, || false)
            .unwrap();
    }
    let mut expected = [[0.0; 2]; 64];
    if loop_live {
        expected[27] = fixture.samples[0].map(|sample| sample * 0.625);
        expected[35] = fixture.samples[8].map(|sample| sample * (1.5 * 0.75));
        expected[58] = fixture.samples[31].map(|sample| sample * (3.0 * 1.125));
    } else if loop_seek {
        expected[32] = fixture.samples[0].map(|sample| sample * 0.75);
        expected[40] = fixture.samples[8].map(|sample| sample * (1.5 * 0.875));
        expected[63] = fixture.samples[31].map(|sample| sample * (3.0 * 1.125));
    } else {
        expected[16] = fixture.samples[0].map(|sample| sample * 0.5);
        expected[24] = fixture.samples[8].map(|sample| sample * (1.5 * 0.625));
        expected[47] = fixture.samples[31].map(|sample| sample * (3.0 * 0.875));
    }
    assert_eq!(output, expected);
    let proof = runtime.dispose_on_control().unwrap();
    let mut anchors = [Some(anchor), retired_anchor];
    proof.record_anchor_cells_on_control(&mut anchors).unwrap();
    if let Some(mut old) = retired_graph {
        old.dispose_on_control()
            .unwrap()
            .record_anchor_cells_on_control(&mut anchors)
            .unwrap();
    }
    for anchor in anchors.iter_mut().flatten() {
        anchor.finish_disposal_on_control().unwrap();
    }
    let current = workers.last_mut().unwrap().quiesce_on_control().unwrap();
    assert_eq!(current.completed_blocks(), 8);
    assert!(current.retire_on_control().unwrap().status.success());
    if loop_live {
        assert!(
            workers[0].transport_is_live_on_control(),
            "outgoing worker faulted after the edit"
        );
        let old = workers[0].quiesce_on_control().unwrap();
        assert!(old.retire_on_control().unwrap().status.success());
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
#[cfg(feature = "allocation-probe")]
fn ordinary_resident_compiler_drives_signed_vst3_automation() {
    use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime};

    const NAME: &str =
        "plugin_effects::tests::ordinary_resident_compiler_drives_signed_vst3_automation";
    const CHILD: &str = "HEX_ORDINARY_AUTHORED_EFFECT_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "91")
            .output()
            .unwrap();
        assert!(
            result.status.success() && String::from_utf8_lossy(&result.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }

    let mut fixture = fixture_with_authored_parameter(2, false, Some(7));
    fixture.samples[8] = [0.25, -0.125];
    fixture.samples[29] = [-0.25, 0.125];
    *fixture.media.values_mut().next().unwrap() =
        hex_engine::Audio::new(48_000, fixture.samples.clone()).unwrap();
    fixture
        .editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device: fixture.device,
            target: hex_project::DeviceAutomationTarget::Vst3 { parameter: 7 },
            points: [
                (BeatTime::ZERO, 0.25, AutomationInterpolation::Linear),
                (
                    BeatTime::from_ratio(1, 1000).unwrap(),
                    0.625,
                    AutomationInterpolation::Step,
                ),
                (
                    BeatTime::from_ratio(1, 800).unwrap(),
                    0.75,
                    AutomationInterpolation::Step,
                ),
            ]
            .map(|(beat, value, outgoing)| AutomationPoint {
                beat,
                value: AutomationValue::new(value).unwrap(),
                outgoing,
            })
            .to_vec(),
        })
        .unwrap();
    let path = std::env::temp_dir().join(format!("hex-ordinary-vst3-{}.hex", std::process::id()));
    hex_storage::save_new(&path, fixture.editor.project()).unwrap();
    let reopened = hex_storage::load(&path);
    std::fs::remove_file(path).unwrap();
    let project = reopened.unwrap();
    assert_eq!(&project, fixture.editor.project());
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let store = hex_engine::native_device_resources::ResidentDeviceAssets::default();
    let resources =
        hex_engine::native_device_resources::PreparedDeviceResources::prepare_on_control(
            &store, &project, 20_000_000, &cancel,
        )
        .unwrap();
    let definition = NativeEffectDefinition::new(std::slice::from_ref(&fixture.effect)).unwrap();
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let compiler = hex_engine::AudioCandidateEffects::new(&registry);
    let revision = Arc::new(
        compiler
            .prepare_registered_revision(
                &project,
                &fixture.media,
                Arc::new(hex_engine::SineTable::new()),
                None,
                &hex_engine::audio_transform::AudioTransformPreparation::default(),
                &resources,
                0,
                &cancel,
            )
            .unwrap(),
    );
    let mut runtime =
        hex_engine::SessionPlayback::new_with_device_resources(revision, &resources).unwrap();
    let (mut workers, mut endpoints) =
        NativeEffectCallbacks::from_effects_on_control(vec![fixture.effect]).unwrap();
    runtime
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    let mut output = [[0.0; 2]; 64];
    let mut rendered = 0;
    for frames in output.chunks_mut(5) {
        let measured = allocation_counter::measure(|| {
            endpoints.begin_host_callback(frames.len()).unwrap();
            runtime
                .render_with_foreign_processors(frames, None, None, None, &mut endpoints)
                .unwrap();
            endpoints.finish_host_callback(true).unwrap();
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        rendered += frames.len();
        let deadline = Instant::now() + Duration::from_secs(2);
        while workers[0].published_blocks_on_control() < rendered as u64 / 8 {
            assert!(Instant::now() < deadline, "signed authored PCM completion");
            std::thread::yield_now();
        }
    }
    let mut expected = [[0.0; 2]; 64];
    expected[16] = fixture.samples[0].map(|sample| sample * 0.5);
    expected[24] = fixture.samples[8].map(|sample| sample * (1.5 * 0.625));
    expected[45] = fixture.samples[29].map(|sample| sample * (2.5 * 0.875));
    expected[47] = fixture.samples[31].map(|sample| sample * (3.0 * 0.875));
    assert_eq!(output, expected);
    let retired = workers[0].quiesce_on_control().unwrap();
    assert_eq!(retired.completed_blocks(), 8);
    assert!(retired.retire_on_control().unwrap().status.success());
}
