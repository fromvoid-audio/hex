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

struct Fixture {
    editor: Editor,
    media: BTreeMap<hex_project::AssetId, hex_engine::Audio>,
    effect: NativeEffect,
    device: hex_project::DeviceId,
    samples: Vec<[f32; 2]>,
}
fn fixture(location: usize, bypassed: bool) -> Fixture {
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
    let effect = NativeEffect::prepare_on_control(
        identity,
        original_instance_for_binding(Binding {
            root: 1,
            instance: identity.device_id(),
            graph: 3,
            epoch: 4,
        }),
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
            let (mut worker, mut callback) = f.effect.into_parts();
            assert!(callback.matches_worker(&worker));
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
        let expected: [[f32; 2]; 8] = std::array::from_fn(|i| {
            let gain = if location == 1 { 2.0 } else { 1.0 };
            [f.samples[i][0] * gain, f.samples[i][1] * gain]
        });
        assert_eq!(
            output, expected,
            "bypass uses the original dry source, not earlier wet input trim"
        );
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
        bypassed: false,
        input_gain: 1.0,
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
