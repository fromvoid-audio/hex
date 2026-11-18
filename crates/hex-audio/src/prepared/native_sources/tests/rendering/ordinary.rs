//! Actual ordinary resource publication must retain its native source, just as
//! constructor publication does. Compiler facts alone are not lifetime custody.
use super::*;
use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
use std::collections::BTreeMap;

mod capture;
mod editor_delivery;
mod fenced_inheritance;
mod manual_base;
mod manual_order;
mod manual_origin;
mod manual_pause;
mod manual_preroll;
mod parameter_extension;
mod parameter_pressure;
mod parameter_publication;
mod paused_monitor;
mod pressure;
#[cfg(not(debug_assertions))]
mod profile;
mod sampler;

fn limits() -> hex_engine::AudioStorageLedgerReservation {
    hex_engine::AudioStorageLedgerReservation {
        data_rows: 4096,
        handle_rows: 4096,
        root_rows: 128,
        max_retained_bytes: 32 * 1024 * 1024,
        max_construction_peak_bytes: 32 * 1024 * 1024,
    }
}

struct OrdinaryFixture {
    control: Control,
    data: Guard<NativeCallbackOwner>,
    errors: Guard<ErrorCallbackOwner>,
    retirement: NativeRetirement,
    binding: Binding,
    editor: Editor,
    media: BTreeMap<hex_project::AssetId, hex_engine::Audio>,
    resources: PreparedDeviceResources,
}
fn fixture(profile: &str) -> OrdinaryFixture {
    fixture_at(profile, 2)
}
fn fixture_at(profile: &str, location: usize) -> OrdinaryFixture {
    fixture_with_quantum(profile, location, 8)
}
fn fixture_with_quantum(profile: &str, location: usize, quantum: u16) -> OrdinaryFixture {
    fixture_with_authored(profile, location, quantum, &[])
}

fn fixture_with_authored(
    profile: &str,
    location: usize,
    quantum: u16,
    ids: &[u32],
) -> OrdinaryFixture {
    fixture_with_authored_mode(profile, location, quantum, ids, false)
}

fn fixture_with_authored_mode(
    profile: &str,
    location: usize,
    quantum: u16,
    ids: &[u32],
    instrument: bool,
) -> OrdinaryFixture {
    fixture_with_authored_mode_and_capacity(profile, location, quantum, ids, instrument, 2)
}

fn fixture_with_authored_mode_and_capacity(
    profile: &str,
    location: usize,
    quantum: u16,
    ids: &[u32],
    instrument: bool,
    source_capacity: usize,
) -> OrdinaryFixture {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .configure_plugin_callback_quantum(&mut callback, quantum)
        .unwrap();
    let (mut data, errors, terminals) =
        crate::native_callback_owner::reserve_native(callback, Arc::clone(&control.status));
    let retirement = NativeRetirement::new(terminals, control.native_candidate_root());
    control.enable_native_custody().unwrap();
    control
        .configure_device_resource_publication_on_control(limits())
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 192 * 1024 * 1024,
                max_construction_peak_data_bytes: 192 * 1024 * 1024,
            },
            3,
            4096,
        )
        .unwrap();
    control
        .configure_native_source_preparation(source_capacity, source_reservation())
        .unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(if instrument {
            Command::AddMidiTrack {
                name: "synth".into(),
            }
        } else {
            Command::AddTrack {
                name: "audio".into(),
            }
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
    let owner = match if instrument { 0 } else { location } {
        0 => ProcessorChainOwner::Track(track.value()),
        1 => ProcessorChainOwner::Return(bus.unwrap().value()),
        _ => ProcessorChainOwner::Main,
    };
    let identity = ProcessorInstanceIdentity::new(owner, editor.project().next_identity()).unwrap();
    use crate::plugin_effects::PluginProfile;
    let binding = source_for_profile_with_authored_mode(
        &mut control,
        identity,
        profile,
        quantum,
        None,
        ids,
        if instrument {
            PluginProfile::Instrument
        } else {
            PluginProfile::Effect
        },
    );
    let state = control.native_source_prepared_state(binding).unwrap();
    let Created::Device(device) = editor
        .execute(if instrument {
            Command::InsertInstrument { track, state }
        } else {
            match location {
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
            }
        })
        .unwrap()
    else {
        panic!("original source")
    };
    for &id in ids {
        editor
            .execute(Command::ReplaceDeviceAutomationTarget {
                device,
                target: hex_project::DeviceAutomationTarget::Vst3 { parameter: id },
                points: vec![hex_project::AutomationPoint {
                    beat: hex_project::BeatTime::ZERO,
                    value: hex_project::AutomationValue::new(if id == 7 { 0.25 } else { 0.5 })
                        .unwrap(),
                    outgoing: hex_project::AutomationInterpolation::Step,
                }],
            })
            .unwrap();
    }
    let media = if instrument {
        let duration = hex_project::BeatTime::from_ratio(44, 24_000).unwrap();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("midi clip");
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration,
                pitch: 60,
                channel: 2,
                velocity: 127,
            })
            .unwrap();
        BTreeMap::new()
    } else {
        let frames = if ids.is_empty() { 32 } else { 96 };
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "ordinary-original.wav".into(),
                sample_rate: 48_000,
                frames,
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
                duration: frames,
                source_offset: 0,
                source_length: frames,
                looping: false,
            })
            .unwrap();
        BTreeMap::from([(
            asset,
            hex_engine::Audio::new(48_000, vec![[0.25, -0.5]; frames as usize]).unwrap(),
        )])
    };
    let resources = PreparedDeviceResources::prepare_on_control(
        &ResidentDeviceAssets::default(),
        editor.project(),
        1_000_000,
        &AtomicBool::new(false),
    )
    .unwrap();
    control.publish_native_source_endpoint(binding).unwrap();
    allocation_free(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    OrdinaryFixture {
        control,
        data,
        errors,
        retirement,
        binding,
        editor,
        media,
        resources,
    }
}
fn compile(f: &OrdinaryFixture, with_native: bool) -> Playback {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let result = crate::plugin_effects::Hosting::prepare_on_control(
            &f.control,
            if with_native {
                std::slice::from_ref(&f.binding)
            } else {
                &[]
            },
            &[],
            &mut |effects| {
                effects.prepare_registered_revision(
                    f.editor.project(),
                    &f.media,
                    Arc::new(hex_engine::SineTable::new()),
                    None,
                    &hex_engine::audio_transform::AudioTransformPreparation::default(),
                    &f.resources,
                    1_000_000,
                    &AtomicBool::new(false),
                    None,
                )
            },
        )
        .map(|revision| {
            Playback::try_from_prepared_with_device_resources(Arc::new(revision), &f.resources)
                .unwrap()
        });
        match result {
            Ok(playback) => return playback,
            Err(reason)
                if matches!(
                    reason.as_str(),
                    "native effect preparation is busy" | "native effect selection is busy"
                ) =>
            {
                assert!(Instant::now() < deadline, "original source compiler borrow");
                std::thread::yield_now();
            }
            Err(reason) => panic!("original source compiler: {reason}"),
        }
    }
}
fn context() -> hex_engine::AudioSourceContext {
    hex_engine::AudioSourceContext {
        project_generation: 41,
        editor_revision: 1,
    }
}
fn publish(control: &mut Control, playback: Playback, policy: PublicationPolicy) -> u64 {
    publish_in_context(control, playback, policy, context())
}
fn publish_in_context(
    control: &mut Control,
    playback: Playback,
    policy: PublicationPolicy,
    source_context: hex_engine::AudioSourceContext,
) -> u64 {
    let mut playback = Some(playback);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match control.publish_with_admission_and_source_on_control(
            playback.take().unwrap(),
            policy,
            source_context,
            |_| Ok::<_, ()>(()),
        ) {
            Ok(revision) => return revision,
            Err(AdmittedPublishError::Publication(refused))
                if refused.reason == "native graph preparation is busy" =>
            {
                playback = Some(refused.playback);
                assert!(
                    Instant::now() < deadline,
                    "ordinary native publication Work lane"
                );
                std::thread::yield_now();
            }
            Err(_) => panic!("actual ordinary native publication"),
        }
    }
}
fn block(f: &mut OrdinaryFixture) -> [Frame; 8] {
    physical_block::<8>(f)
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn retained_foreign_track_tail_and_successor_use_distinct_original_workers() {
    retained_foreign_track_tail(false, false, false, false);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn retained_foreign_track_tail_routes_through_unchanged_compensated_group() {
    retained_foreign_track_tail(true, false, false, false);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn silent_group_child_edits_keep_the_original_instrument_worker_and_gate() {
    const CHILD: &str = "HEX_COLD_GROUP_CHILD_SDK_DRIVER";
    if std::env::var_os(CHILD).is_none() {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
            ])
            .env(CHILD, "1")
            .env("RUST_MIN_STACK", "16777216")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "146")
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
    let _serial = serial();
    let mut edited = fixture_with_authored_mode("processing", 0, 8, &[], true);
    let mut reference = fixture_with_authored_mode("processing", 0, 8, &[], true);
    let track = edited.editor.project().tracks()[0].id();
    for f in [&mut edited, &mut reference] {
        f.editor
            .execute(Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
                name: "group".into(),
            })
            .unwrap();
        let playback = compile(f, true);
        assert!(playback.main_output_latency_frames() > 0);
        publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
        f.control.command(PlaybackCommand::Play).unwrap();
    }
    let render = |f: &mut OrdinaryFixture| {
        let output = block(f);
        let mut frames = None;
        f.data.owner_mut().unwrap().run(|callback, _| {
            frames = callback.native_sources.test_committed_frames(f.binding);
        });
        replacement::wait_for_blocks(
            &f.control.native_candidate_root(),
            f.binding,
            frames.unwrap() / 8,
        );
        output
    };
    let mut pcm = [[0.0; 2]; 80];
    let mut empty = None;
    for (index, chunk) in pcm.chunks_exact_mut(8).enumerate() {
        let edit = match index {
            3 => {
                let Created::Track(track) = edited
                    .editor
                    .execute(Command::InsertTrack {
                        name: "empty".into(),
                        before_track: Some(track),
                    })
                    .unwrap()
                else {
                    panic!("inserted track");
                };
                empty = Some(track);
                true
            }
            4 => {
                edited
                    .editor
                    .execute(Command::RemoveTrack(empty.unwrap()))
                    .unwrap();
                true
            }
            _ => false,
        };
        let revision = edit.then(|| {
            let playback = compile(&edited, true);
            publish(
                &mut edited.control,
                playback,
                PublicationPolicy::InheritRuntime,
            )
        });
        chunk.copy_from_slice(&render(&mut edited));
        assert_eq!(
            chunk,
            render(&mut reference),
            "original SDK quantum {index}"
        );
        if let Some(revision) = revision {
            assert_eq!(
                edited.control.status.applied_revision.load(Acquire),
                revision
            );
        }
        assert!(edited.control.status.rejected_revision().is_none());
        assert!(edited.control.status.render_fault_snapshot().is_none());
        assert!(edited.control.status.playing.load(Acquire));
    }
    assert!(pcm[24..40].iter().all(|frame| frame[0].abs() > 0.001));
    assert!(pcm[64..].iter().all(|frame| *frame == [0.0; 2]));
    for f in [edited, reference] {
        assert!(
            matches!(dispose(f).disposal, native::PreparationDisposal::Reaped(retired) if retired.status.success())
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn retained_foreign_track_tail_preserves_direct_send_through_group() {
    retained_foreign_track_tail(true, true, false, false);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn overlapping_foreign_tails_keep_their_original_workers() {
    retained_foreign_track_tail(true, true, true, false);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn unbounded_foreign_tails_keep_original_workers_through_overlap_and_shutdown() {
    const CHILD: &str = "HEX_UNBOUNDED_TAIL_DRIVER";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
            ])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_TAIL_FRAMES", u32::MAX.to_string())
            .output()
            .unwrap();
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    retained_foreign_track_tail(true, true, true, true);
}

fn retained_foreign_track_tail(
    grouped: bool,
    direct_send: bool,
    overlap: bool,
    expect_unbounded: bool,
) {
    let _serial = serial();
    let mut f = if overlap {
        fixture_with_authored_mode_and_capacity("processing", 0, 8, &[], false, 3)
    } else {
        fixture_at("processing", 0)
    };
    let old = f.binding;
    let track = f.editor.project().tracks()[0].id();
    let device = f.editor.project().tracks()[0].devices()[0].id();
    f.editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    if grouped {
        let Created::Track(group) = f
            .editor
            .execute(Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        f.editor
            .execute(Command::Gain {
                track: group,
                gain: 0.5,
            })
            .unwrap();
    }
    if direct_send {
        let Created::ReturnTrack(bus) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        f.editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 0.5,
            })
            .unwrap();
    }
    let first = compile(&f, true);
    if expect_unbounded {
        assert!(first.foreign_nodes_on_control().any(|node| {
            matches!(&node.definition, Some(hex_lattice::PreparedProcessorFactory::Foreign(prepared)) if prepared.binding() == Some(old) && prepared.output_tail() == hex_lattice::OutputTail::Unbounded)
        }));
    }
    publish(&mut f.control, first, PublicationPolicy::InheritRuntime);
    f.control.command(PlaybackCommand::Play).unwrap();
    let root = f.control.native_candidate_root();
    block(&mut f);
    replacement::wait_for_blocks(&root, old, 1);
    let saved = f.control.native_source_prepared_state(old).unwrap();
    let state = hex_plugin_state::decode(&saved).unwrap();
    let mut component = state.component.unwrap().to_vec();
    component[8..12].copy_from_slice(&0.75_f32.to_le_bytes());
    let restored = hex_plugin_state::encode(hex_plugin_state::State {
        component: Some(&component),
        ..state
    })
    .unwrap();
    let identity =
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(track.value()), device.value())
            .unwrap();
    let next = source_for_profile(&mut f.control, identity, "processing", 8, Some(restored));
    assert_ne!(old, next);
    let captured = f.control.native_source_prepared_state(next).unwrap();
    f.editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: captured.clone(),
        })
        .unwrap();
    f.control.publish_native_source_endpoint(next).unwrap();
    allocation_free(|| {
        f.data
            .owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    f.binding = next;
    let successor = compile(&f, true);
    let mut revision = publish(&mut f.control, successor, PublicationPolicy::InheritRuntime);
    let mut last = None;
    if overlap {
        assert_eq!(block(&mut f), [[0.0; 2]; 8]);
        replacement::wait_for_blocks(&root, old, 2);
        replacement::wait_for_blocks(&root, next, 1);
        assert_eq!(f.control.status.applied_revision.load(Acquire), revision);
        let third = source_for_profile(&mut f.control, identity, "processing", 8, Some(captured));
        assert_ne!(third, old);
        assert_ne!(third, next);
        f.control.publish_native_source_endpoint(third).unwrap();
        allocation_free(|| {
            f.data
                .owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
        f.binding = third;
        let successor = compile(&f, true);
        revision = publish(&mut f.control, successor, PublicationPolicy::InheritRuntime);
        last = Some(third);
    }
    let mut output = [[0.0; 2]; 32];
    for (index, chunk) in output.chunks_mut(8).enumerate() {
        chunk.copy_from_slice(&block(&mut f));
        if index == 0 && !overlap {
            replacement::wait_for_blocks(&root, old, 2);
        }
        if let Some(third) = last {
            if index == 0 {
                replacement::wait_for_blocks(&root, old, 3);
                replacement::wait_for_blocks(&root, next, 2);
            }
            replacement::wait_for_blocks(&root, third, index as u64 + 1);
        } else {
            replacement::wait_for_blocks(&root, next, index as u64 + 1);
        }
    }
    assert_eq!(f.control.status.applied_revision.load(Acquire), revision);
    let gain = if grouped && !direct_send { 0.5 } else { 1.0 };
    if overlap {
        assert_eq!(&output[..8], &[[0.125 * gain, -0.25 * gain]; 8]);
        assert_eq!(&output[8..16], &[[0.09375 * gain, -0.1875 * gain]; 8]);
    } else {
        assert_eq!(&output[..8], &[[0.0; 2]; 8]);
        assert_eq!(&output[8..16], &[[0.125 * gain, -0.25 * gain]; 8]);
        assert_eq!(&output[16..24], &[[0.09375 * gain, -0.1875 * gain]; 8]);
        assert_eq!(&output[24..], &[[0.1171875 * gain, -0.234375 * gain]; 8]);
    }
    let OrdinaryFixture {
        mut control,
        data,
        errors,
        retirement,
        ..
    } = f;
    let mut retirement = close_backend(data, errors, retirement);
    for binding in [Some(old), Some(next), last].into_iter().flatten() {
        let returned = recover(&mut control, binding);
        assert!(returned.error.is_none() && returned.cleanup_error.is_none());
        assert!(
            matches!(returned.disposal, native::PreparationDisposal::Reaped(retired) if retired.status.success())
        );
    }
    drop(control);
    finish(&mut retirement);
}

fn physical_block<const FRAMES: usize>(f: &mut OrdinaryFixture) -> [Frame; FRAMES] {
    let mut output = [[0.0; 2]; FRAMES];
    let mut host = [0.0_f32; 512];
    allocation_free(|| {
        f.data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.begin_native_effect_callback(FRAMES).unwrap();
            let rendered = callback.render(&mut output);
            assert!(rendered.is_ok());
            assert!(crate::device::finish_host_chunk(
                callback,
                &mut host[..FRAMES * 2],
                &mut output,
                2,
                &rendered,
                0,
                Instant::now()
            ));
        })
    });
    output
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_paused_vst3_instrument_auditions_on_its_original_worker() {
    const CHILD: &str = "HEX_PAUSED_VST3_INSTRUMENT_SDK_DRIVER";
    if std::env::var_os(CHILD).is_none() {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
            ])
            .env(CHILD, "1")
            .env("RUST_MIN_STACK", "16777216")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "146")
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
    use hex_engine::computer_keyboard::ComputerKeyboardInput;
    use hex_lattice_contract::DeviceTypeId;
    const QUANTUM: usize = 256;
    let _serial = serial();
    let mut f = fixture_with_authored_mode("processing", 0, QUANTUM as u16, &[], true);
    f.editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.limiter",
            ))
            .unwrap(),
        })
        .unwrap();
    let keys = Arc::new(ComputerKeyboardInput::default());
    let mut playback = compile(&f, true);
    let track = f.editor.project().tracks()[0].id();
    playback
        .prepare_computer_keyboard(f.editor.project(), track, Arc::clone(&keys))
        .unwrap();
    assert!(
        Playback::paused_source_monitor_allowance_on_control(
            playback.prepared_revision().unwrap(),
            Some(&f.resources),
        )
        .unwrap()
            > 0
    );
    playback
        .prepare_resident_paused_source_monitor(Some(&f.resources))
        .unwrap();
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    let root = f.control.native_candidate_root();
    let render = |f: &mut OrdinaryFixture| {
        let output = physical_block::<QUANTUM>(f);
        let mut committed = None;
        f.data.owner_mut().unwrap().run(|callback, _| {
            committed = callback.native_sources.test_committed_frames(f.binding);
        });
        replacement::wait_for_blocks(&root, f.binding, committed.unwrap() / QUANTUM as u64);
        output
    };
    f.control.command(PlaybackCommand::Play).unwrap();
    render(&mut f);
    f.control.command(PlaybackCommand::Pause).unwrap();
    render(&mut f);
    assert!(!f.control.status.playing.load(Acquire));
    let frozen = f.control.status.position.load(Acquire);
    assert!(frozen > 0);
    for _ in 0..5 {
        render(&mut f);
    }
    assert_eq!(render(&mut f), [[0.0; 2]; QUANTUM]);
    keys.set_key(0, Some((67, 100, 1))).unwrap();
    let mut heard = false;
    for _ in 0..6 {
        heard |= render(&mut f)
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.001);
    }
    assert!(heard, "the original worker must render the paused key");
    assert_eq!(f.control.status.position.load(Acquire), frozen);
    assert_eq!(
        f.control.native_source_endpoint_status(f.binding),
        Some((true, true))
    );
    keys.set_key(0, None).unwrap();
    for _ in 0..5 {
        render(&mut f);
    }
    assert_eq!(render(&mut f), [[0.0; 2]; QUANTUM]);
    f.control.command(PlaybackCommand::Play).unwrap();
    render(&mut f);
    assert!(f.control.status.playing.load(Acquire));
    f.control.command(PlaybackCommand::Stop).unwrap();
    render(&mut f);
    assert_eq!(f.control.status.position.load(Acquire), 0);
    let returned = dispose(f);
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(retired) if retired.status.success())
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_pause_keeps_the_vst3_source_available_for_play_from() {
    let _serial = serial();
    // The 128-frame Pause fade ends inside this physical callback.
    const QUANTUM: usize = 256;
    let mut f = fixture_with_quantum("processing", 0, QUANTUM as u16);
    let playback = compile(&f, true);
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    let root = f.control.native_candidate_root();
    let render = |f: &mut OrdinaryFixture| {
        let output = physical_block::<QUANTUM>(f);
        let mut committed = None;
        f.data.owner_mut().unwrap().run(|callback, _| {
            committed = callback.native_sources.test_committed_frames(f.binding);
        });
        replacement::wait_for_blocks(&root, f.binding, committed.unwrap() / QUANTUM as u64);
        output
    };
    f.control.command(PlaybackCommand::Play).unwrap();
    render(&mut f);
    let source = f.control.status.native_effect_source().unwrap();

    f.control.command(PlaybackCommand::Pause).unwrap();
    render(&mut f);
    assert!(!f.control.status.playing.load(Acquire));
    assert_eq!(
        f.control.native_source_endpoint_status(f.binding),
        Some((true, true))
    );

    f.control.command(PlaybackCommand::PlayFrom(0)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        f.control.poll_resident_publication();
        render(&mut f);
        if f.control.status.playing.load(Acquire) {
            break;
        }
        assert!(Instant::now() < deadline, "PlayFrom did not resume");
        std::thread::yield_now();
    }
    assert_eq!(f.control.status.native_effect_source(), Some(source));
    // The copied endpoint returns the restarted input two quanta later.
    render(&mut f);
    let restarted = render(&mut f);
    assert!(restarted.iter().flatten().any(|&sample| sample != 0.0));
    let returned = dispose(f);
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(retired) if retired.status.success())
    );
}
fn dispose(f: OrdinaryFixture) -> crate::NativeSourcePreparationReturn {
    let OrdinaryFixture {
        mut control,
        data,
        errors,
        retirement,
        binding,
        ..
    } = f;
    let mut retirement = close_backend(data, errors, retirement);
    let returned = recover(&mut control, binding);
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    drop(control);
    finish(&mut retirement);
    returned
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn native_fixture_retries_the_original_endpoint_return_after_work_contention() {
    let _serial = serial();
    let mut f = fixture("processing");
    let playback = compile(&f, true);
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    f.control.command(PlaybackCommand::Play).unwrap();
    block(&mut f);
    replacement::wait_for_blocks(&f.control.native_candidate_root(), f.binding, 1);
    let OrdinaryFixture {
        mut control,
        data,
        errors,
        mut retirement,
        binding,
        ..
    } = f;
    let root = control.native_candidate_root();
    drop(data);
    drop(errors);
    assert!(!retirement.advance().unwrap());
    root.with_native_source_work_held_on_control(|| {
        let mut backend = Some(());
        let mut pending = false;
        let mut disposed = false;
        assert!(
            !crate::lifecycle::test_dispose_backend(
                &mut backend,
                &mut pending,
                &mut disposed,
                &mut retirement,
                |_| Ok(())
            )
            .unwrap()
        );
        assert!(disposed && backend.is_none());
        assert!(!retirement.test_native_source_carriers_returned());
        assert_eq!(retirement.test_native_source_carriers_pending(), 1);
    })
    .unwrap();
    finish_source_carrier_return(&mut retirement);
    let returned = recover(&mut control, binding);
    assert_eq!(returned.binding, binding);
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
        if retired.status.success() && retired.terminal.is_some())
    );
    drop(control);
    finish(&mut retirement);
}

fn renew_cohort(f: &mut OrdinaryFixture) {
    let next = epoch::renew_after_returns(&mut f.control, f.binding);
    epoch::on_control(|| {
        f.control.complete_native_source_preparation(
            next,
            Some(hex_plugin_host::protocol::PresentationLatency {
                input_frames: 77,
                output_frames: 91,
            }),
        )
    });
    epoch::on_control(|| f.control.start_native_effect(next));
    f.binding = next;
    epoch::on_control(|| f.control.publish_native_source_endpoint(next));
    allocation_free(|| {
        f.data
            .owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_second_stop_cannot_install_an_already_revoked_native_successor() {
    let _serial = serial();
    let mut f = fixture("processing");
    let root = f.control.native_candidate_root();
    let playback = compile(&f, true);
    let original = publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    f.control.command(PlaybackCommand::Play).unwrap();
    for n in 0..3 {
        block(&mut f);
        replacement::wait_for_blocks(&root, f.binding, n + 1);
    }
    let source = f.control.status.native_effect_source().unwrap();
    f.control.stop();
    let fence = f.control.request_native_epoch_fence(source).unwrap();
    allocation_free(|| {
        f.data
            .owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    f.control
        .return_stopped_ordinary_native_epoch_runtime(source, fence)
        .unwrap();
    for _ in 0..2 {
        allocation_free(|| {
            f.data
                .owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
    }
    renew_cohort(&mut f);
    let playback = compile(&f, true);
    let candidate = publish(
        &mut f.control,
        playback,
        PublicationPolicy::InstallUnderNativeEpoch(fence),
    );
    // Queue acceptance preceded Stop; the callback must still refuse this
    // now-revoked pair rather than install a graph that cannot process on Play.
    f.control.stop();
    let mut output = [[9.0; 2]; 8];
    let mut host = [9.0_f32; 16];
    allocation_free(|| {
        f.data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.begin_native_effect_callback(8).unwrap();
            let rendered = callback.render(&mut output);
            assert!(rendered.is_ok());
            let _accepted = crate::device::finish_host_chunk(
                callback,
                &mut host,
                &mut output,
                2,
                &rendered,
                0,
                Instant::now(),
            );
        })
    });
    assert_eq!(output, [[0.0; 2]; 8]);
    let applied = f.control.status.applied_revision.load(Acquire);
    let rejection = f.control.status.rejected_revision();
    if applied != original
        || rejection
            != Some((
                candidate,
                hex_engine::PlaybackHandoffRejection::RuntimeFenceChanged,
            ))
    {
        dispose(f);
        panic!(
            "second Stop installed a revoked successor: applied={applied} original={original} rejection={rejection:?}"
        );
    }
    f.control.reclaim();
    // Preserve the actual same child after this real refusal. The output has
    // no graph, but the revoked unused cohort must return before a fresh ACK.
    f.control
        .return_stopped_ordinary_native_epoch_runtime(source, fence)
        .unwrap();
    for _ in 0..2 {
        allocation_free(|| {
            f.data
                .owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
    }
    renew_cohort(&mut f);
    let playback = compile(&f, true);
    let resumed = publish(
        &mut f.control,
        playback,
        PublicationPolicy::InstallUnderNativeEpoch(fence),
    );
    assert_eq!(block(&mut f), [[0.0; 2]; 8]);
    assert_eq!(f.control.status.applied_revision.load(Acquire), resumed);
    f.control.release_stopped_edit_fence(fence);
    f.control.command(PlaybackCommand::Play).unwrap();
    for n in 0..3 {
        let output = block(&mut f);
        replacement::wait_for_blocks(&root, f.binding, n + 4);
        assert_eq!(
            output,
            if n < 2 {
                [[0.0; 2]; 8]
            } else {
                [[0.21875, -0.4375]; 8]
            }
        );
    }
    let returned = dispose(f);
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
        if retired.status.success() && retired.terminal.is_some_and(|terminal| (terminal.blocks, terminal.frames) == (6, 48)))
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn ordinary_publication_reports_unavailable_source_without_losing_candidate() {
    // The existing original-SDK fixture exceeds the default debug test stack.
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let _serial = serial();
            let mut f = fixture("processing");
            let initial = compile(&f, true);
            let revision = publish(&mut f.control, initial, PublicationPolicy::InheritRuntime);
            block(&mut f);
            let candidate = compile(&f, true);
            let prepared = Arc::clone(candidate.prepared_revision().unwrap());
            let candidate = match f.control.publish_with_policy_and_context_generation(
                candidate,
                PublicationPolicy::InheritRuntime,
                41,
            ) {
                Err(refused) => {
                    assert_eq!(refused.kind, crate::prepared::PublishErrorKind::Other);
                    assert_eq!(
                        refused.reason,
                        "native graph publication requires a canonical source context"
                    );
                    refused.playback
                }
                Ok(_) => panic!("native graph must have its source context"),
            };
            let candidate = match f.control.publish_with_admission_and_source_on_control(
                candidate,
                PublicationPolicy::InheritRuntime,
                context(),
                |_| Err::<(), _>("admission refusal"),
            ) {
                Err(AdmittedPublishError::Admission { reason, playback }) => {
                    assert_eq!(reason, "admission refusal");
                    playback
                }
                _ => panic!("unrelated admission must remain distinct"),
            };
            allocation_free(|| {
                f.data
                    .owner_mut()
                    .unwrap()
                    .run(|callback, _| callback.revoke_native_effects())
            });
            let mut candidate = Some(candidate);
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match f.control.publish_with_admission_and_source_on_control(
                    candidate.take().unwrap(),
                    PublicationPolicy::InheritRuntime,
                    context(),
                    |_| Ok::<_, ()>(()),
                ) {
                    Err(AdmittedPublishError::Publication(refused))
                        if refused.reason == "native graph preparation is busy" =>
                    {
                        candidate = Some(refused.playback);
                        assert!(Instant::now() < deadline);
                        std::thread::yield_now();
                    }
                    Err(AdmittedPublishError::Publication(refused)) => {
                        assert_eq!(
                            refused.kind,
                            crate::prepared::PublishErrorKind::NativeSourceUnavailable
                        );
                        assert_eq!(refused.reason, "native graph source is unavailable");
                        assert!(Arc::ptr_eq(
                            refused.playback.prepared_revision().unwrap(),
                            &prepared
                        ));
                        assert_eq!(f.control.submitted_revision(), revision);
                        break;
                    }
                    _ => panic!("revoked source must refuse publication"),
                }
            }
            dispose(f);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_native_graph_retains_endpoint_until_its_original_graph_return() {
    let _serial = serial();
    let mut f = fixture("processing");
    let playback = compile(&f, true);
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    f.control.command(PlaybackCommand::Play).unwrap();
    let root = f.control.native_candidate_root();
    for n in 0..3 {
        let output = block(&mut f);
        replacement::wait_for_blocks(&root, f.binding, n + 1);
        assert_eq!(
            output,
            if n < 2 {
                [[0.0; 2]; 8]
            } else {
                [[0.125, -0.25]; 8]
            }
        );
    }
    assert!(f.control.retire_native_source_preparation(f.binding));
    let mut kept_endpoint = false;
    allocation_free(|| {
        f.data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            assert!(matches!(
                callback.active,
                Some(ActivePublication::Legacy(_))
            ));
            kept_endpoint = callback.has_native_source_endpoints();
        })
    });
    // Clean actual owners even when testing the pre-fix failure.
    dispose(f);
    assert!(
        kept_endpoint,
        "source retirement returned the endpoint while its ordinary graph was still current"
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_stop_returns_graph_and_endpoints_before_same_child_epoch_pcm() {
    let _serial = serial();
    for location in 0..3 {
        ordinary_epochs(location);
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_native_main_tail_survives_graph_returns_then_reaps_while_backend_lives() {
    let _serial = serial();
    let mut f = fixture("tail");
    let root = f.control.native_candidate_root();
    let playback = compile(&f, true);
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    f.control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 64];
    for n in 0..8 {
        if matches!(n, 1 | 2 | 6) {
            if n == 1 {
                f.editor
                    .execute(Command::RemoveDevice(
                        f.editor.project().main_devices()[0].id(),
                    ))
                    .unwrap();
            } else {
                f.editor
                    .execute(Command::RenameTrack {
                        track: f.editor.project().tracks()[0].id(),
                        name: format!("successor {n}"),
                    })
                    .unwrap();
            }
            let playback = compile(&f, false);
            publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
        }
        output[n * 8..n * 8 + 8].copy_from_slice(&block(&mut f));
        if n <= 4 {
            replacement::wait_for_blocks(&root, f.binding, n as u64 + 1);
        }
        if n == 1 {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !f.control.retire_native_source_preparation(f.binding) {
                assert!(
                    Instant::now() < deadline,
                    "original tail source retirement request"
                );
                std::thread::yield_now();
            }
        }
        // Destroy every returned original graph immediately. Its native tail
        // must remain usable in actual successor runtime custody, rather than
        // relying on a held predecessor or an unprocessed return queue.
        f.control.reclaim();
        if (1..5).contains(&n) {
            assert!(
                f.control
                    .recover_native_source_preparation(f.binding)
                    .is_none()
            );
        }
    }
    let mut expected = [[0.0; 2]; 64];
    expected[8..32].fill([0.25, -0.5]);
    for index in 0..8 {
        expected[index + 16][0] += 0.25 * 0.5;
        expected[index + 16][1] += -0.5 * 0.5;
        expected[index + 19][0] += 0.25 * 0.125;
        expected[index + 19][1] += -0.5 * 0.125;
    }
    assert_eq!(
        output, expected,
        "ordinary successors retain the original FIR and bridge once"
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    let returned = loop {
        allocation_free(|| {
            f.data
                .owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
        f.control.reclaim();
        if let Some(returned) = f.control.recover_native_source_preparation(f.binding) {
            break returned;
        }
        assert!(
            Instant::now() < deadline,
            "actual drained native tail return while backend is live"
        );
        std::thread::yield_now();
    };
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
        if retired.status.success() && retired.terminal.is_some_and(|terminal| (terminal.blocks, terminal.frames) == (5, 40)))
    );
    let OrdinaryFixture {
        control,
        data,
        errors,
        retirement,
        ..
    } = f;
    let mut retirement = close_backend(data, errors, retirement);
    drop(control);
    finish(&mut retirement);
}
fn ordinary_epochs(location: usize) {
    let mut f = fixture_at("processing", location);
    let canonical = f.editor.project().clone();
    let saved = f.control.native_source_prepared_state(f.binding).unwrap();
    let original_pcm = f.media.values().next().unwrap().clone();
    let playback = compile(&f, true);
    let mut revision = publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    let root = f.control.native_candidate_root();
    for epoch_index in 0..3_u64 {
        f.control.command(PlaybackCommand::Play).unwrap();
        for n in 0..3_u64 {
            let output = block(&mut f);
            replacement::wait_for_blocks(&root, f.binding, epoch_index * 3 + n + 1);
            let gain =
                0.5 + epoch_index as f32 * 3.0 * 0.125 + if location == 1 { 1.0 } else { 0.0 };
            assert_eq!(
                output,
                if n < 2 {
                    [[0.0; 2]; 8]
                } else {
                    [[0.25 * gain, -0.5 * gain]; 8]
                }
            );
        }
        assert_eq!(f.editor.project(), &canonical);
        assert!(
            f.media
                .values()
                .next()
                .unwrap()
                .shares_pcm_with(&original_pcm)
        );
        assert_eq!(
            f.control.native_source_prepared_state(f.binding),
            Some(saved.clone())
        );
        assert!(
            f.control.status.applied_audio_source().is_none(),
            "ordinary graphs must not masquerade as constructors or private samplers"
        );
        let source = f
            .control
            .status
            .native_effect_source()
            .expect("actual ordinary native installation");
        assert_eq!(
            source,
            AppliedAudioSource {
                output_identity: root.root_id(),
                audio_revision: revision,
                context: context()
            }
        );
        if epoch_index == 2 {
            break;
        }
        let stale_playback = compile(&f, true);
        let stale_revision = Arc::clone(stale_playback.prepared_revision().unwrap());
        f.control.stop();
        for stale in [
            AppliedAudioSource {
                output_identity: source.output_identity + 1,
                ..source
            },
            AppliedAudioSource {
                audio_revision: source.audio_revision + 1,
                ..source
            },
            AppliedAudioSource {
                context: hex_engine::AudioSourceContext {
                    editor_revision: source.context.editor_revision + 1,
                    ..source.context
                },
                ..source
            },
        ] {
            assert!(
                f.control
                    .request_ordinary_native_epoch_fence(stale)
                    .is_err()
            );
            assert!(!f.control.status.stopped_edit_pending());
        }
        let fence = if epoch_index == 0 {
            f.control.request_native_epoch_fence(source)
        } else {
            f.control.request_ordinary_native_epoch_fence(source)
        }
        .unwrap();
        allocation_free(|| {
            f.data
                .owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
        assert_eq!(
            f.control.stopped_edit_fence_state(fence),
            StoppedEditFenceState::Ready
        );
        assert!(f.control.command(PlaybackCommand::Play).is_err());
        assert!(
            f.control
                .return_stopped_ordinary_native_epoch_runtime(
                    AppliedAudioSource {
                        audio_revision: source.audio_revision + 1,
                        ..source
                    },
                    fence
                )
                .is_err()
        );
        assert!(
            f.control
                .quiesce_native_source_preparation(f.binding, f.binding.graph)
                .is_err()
        );
        f.control
            .return_stopped_ordinary_native_epoch_runtime(source, fence)
            .unwrap();
        // Queue admission is not graph or endpoint return.
        assert!(
            f.control
                .quiesce_native_source_preparation(f.binding, f.binding.graph)
                .is_err()
        );
        allocation_free(|| {
            f.data
                .owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
        assert!(f.control.status.native_effect_source().is_none());
        allocation_free(|| {
            f.data
                .owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
        let next = epoch::renew_after_returns(&mut f.control, f.binding);
        assert_eq!(
            (next.root, next.instance, next.graph),
            (f.binding.root, f.binding.instance, f.binding.graph)
        );
        assert!(next.epoch > f.binding.epoch);
        assert!(f.control.native_source_prepared_state(next).is_none());
        assert!(
            f.control.publish_native_source_endpoint(next).is_err(),
            "no endpoint before actual new native ACK"
        );
        epoch::on_control(|| {
            f.control.complete_native_source_preparation(
                next,
                Some(hex_plugin_host::protocol::PresentationLatency {
                    input_frames: 77,
                    output_frames: 91,
                }),
            )
        });
        epoch::on_control(|| f.control.start_native_effect(next));
        f.binding = next;
        epoch::on_control(|| f.control.publish_native_source_endpoint(next));
        allocation_free(|| {
            f.data
                .owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
        match f.control.publish_with_admission_and_source_on_control(
            stale_playback,
            PublicationPolicy::InstallUnderNativeEpoch(fence),
            context(),
            |_| Ok::<_, ()>(()),
        ) {
            Err(AdmittedPublishError::Publication(refused)) => {
                assert_eq!(refused.reason, "native graph source differs");
                assert!(Arc::ptr_eq(
                    refused.playback.prepared_revision().unwrap(),
                    &stale_revision
                ));
            }
            _ => panic!("old epoch facts cannot admit an ordinary successor"),
        }
        let playback = compile(&f, true);
        revision = publish(
            &mut f.control,
            playback,
            PublicationPolicy::InstallUnderNativeEpoch(fence),
        );
        assert!(f.control.command(PlaybackCommand::Play).is_err());
        // Ordinary source windows install at render, even when their current
        // resident inputs need no pages. Complete the actual paused render and
        // final host acceptance rather than treating begin_block as its ACK.
        assert_eq!(block(&mut f), [[0.0; 2]; 8]);
        assert_eq!(f.control.status.applied_revision.load(Acquire), revision);
        f.control.release_stopped_edit_fence(fence);
        assert!(
            f.control
                .return_stopped_ordinary_native_epoch_runtime(source, fence)
                .is_err()
        );
    }
    let returned = dispose(f);
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
        if retired.status.success() && retired.terminal.is_some_and(|terminal| (terminal.blocks, terminal.frames) == (9, 72)))
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn unbounded_native_return_tail_refuses_unaligned_removal_without_losing_source() {
    unbounded_return_or_main_tail(1);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn unbounded_native_main_tail_is_held_until_stop_and_shutdown() {
    unbounded_return_or_main_tail(2);
}

fn unbounded_return_or_main_tail(location: usize) {
    const CHILD: &str = "HEX_UNBOUNDED_RETURN_TAIL_DRIVER";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
            ])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_TAIL_FRAMES", u32::MAX.to_string())
            .output()
            .unwrap();
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let _serial = serial();
    let mut f = fixture_at("tail", location);
    let root = f.control.native_candidate_root();
    let playback = compile(&f, true);
    assert!(playback.foreign_nodes_on_control().any(|node| matches!(&node.definition, Some(hex_lattice::PreparedProcessorFactory::Foreign(prepared)) if prepared.binding() == Some(f.binding) && prepared.output_tail() == hex_lattice::OutputTail::Unbounded)));
    let original_revision = publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    f.control.command(PlaybackCommand::Play).unwrap();
    block(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 1);
    let device = f
        .editor
        .project()
        .devices()
        .find(|device| device.id().value() == f.binding.instance)
        .unwrap()
        .id();
    f.editor.execute(Command::RemoveDevice(device)).unwrap();
    let successor = compile(&f, false);
    let revision = publish(&mut f.control, successor, PublicationPolicy::InheritRuntime);
    let mut heard_tail = false;
    for n in 2..=5 {
        let output = block(&mut f);
        heard_tail |= output.iter().flatten().any(|sample| *sample != 0.0);
        replacement::wait_for_blocks(&root, f.binding, n);
        f.control.reclaim();
    }
    if location == 1 {
        assert_eq!(
            f.control.status.applied_revision.load(Acquire),
            original_revision
        );
        assert_eq!(
            f.control.status.rejected_revision(),
            Some((revision, PlaybackHandoffRejection::LatencyLayoutChanged))
        );
    } else {
        assert_eq!(f.control.status.applied_revision.load(Acquire), revision);
    }
    assert!(heard_tail, "original endpoint must contribute to output");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !f.control.retire_native_source_preparation(f.binding) {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    f.control.reclaim();
    assert!(
        f.control
            .recover_native_source_preparation(f.binding)
            .is_none()
    );
    f.control.stop();
    block(&mut f);
    let OrdinaryFixture {
        mut control,
        data,
        errors,
        retirement,
        binding,
        ..
    } = f;
    let mut retirement = close_backend(data, errors, retirement);
    let returned = recover(&mut control, binding);
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(retired) if retired.status.success())
    );
    drop(control);
    finish(&mut retirement);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn unbounded_native_return_replacement_refuses_without_losing_original_worker() {
    const CHILD: &str = "HEX_UNBOUNDED_RETURN_REPLACEMENT_DRIVER";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
            ])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_TAIL_FRAMES", u32::MAX.to_string())
            .output()
            .unwrap();
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let _serial = serial();
    let mut f = fixture_at("tail", 1);
    let root = f.control.native_candidate_root();
    let old = f.binding;
    let first = compile(&f, true);
    assert!(first.foreign_nodes_on_control().any(|node| matches!(&node.definition, Some(hex_lattice::PreparedProcessorFactory::Foreign(prepared)) if prepared.binding() == Some(old) && prepared.output_tail() == hex_lattice::OutputTail::Unbounded)));
    let original_revision = publish(&mut f.control, first, PublicationPolicy::InheritRuntime);
    f.control.command(PlaybackCommand::Play).unwrap();
    for n in 1..=4 {
        block(&mut f);
        replacement::wait_for_blocks(&root, old, n);
    }
    let device = f
        .editor
        .project()
        .devices()
        .find(|device| device.id().value() == old.instance)
        .unwrap()
        .id();
    let return_id = f.editor.project().return_tracks()[0].id();
    let identity = ProcessorInstanceIdentity::new(
        ProcessorChainOwner::Return(return_id.value()),
        device.value(),
    )
    .unwrap();
    let saved = f.control.native_source_prepared_state(old).unwrap();
    let next = source_for_profile(&mut f.control, identity, "tail", 8, Some(saved));
    assert_ne!(old, next);
    f.control.publish_native_source_endpoint(next).unwrap();
    allocation_free(|| {
        f.data
            .owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    f.binding = next;
    let successor = compile(&f, true);
    let revision = publish(&mut f.control, successor, PublicationPolicy::InheritRuntime);
    let mut output = [[0.0; 2]; 32];
    for (index, chunk) in output.chunks_mut(8).enumerate() {
        chunk.copy_from_slice(&block(&mut f));
        replacement::wait_for_blocks(&root, old, index as u64 + 5);
    }
    assert_eq!(
        f.control.status.applied_revision.load(Acquire),
        original_revision
    );
    assert_eq!(
        f.control.status.rejected_revision(),
        Some((revision, PlaybackHandoffRejection::SessionRuntime))
    );
    assert!(output.iter().flatten().any(|sample| *sample != 0.0));
    let OrdinaryFixture {
        mut control,
        data,
        errors,
        retirement,
        ..
    } = f;
    let mut retirement = close_backend(data, errors, retirement);
    for binding in [old, next] {
        let returned = recover(&mut control, binding);
        assert!(returned.error.is_none() && returned.cleanup_error.is_none());
        assert!(
            matches!(returned.disposal, native::PreparationDisposal::Reaped(retired) if retired.status.success())
        );
    }
    drop(control);
    finish(&mut retirement);
}
