//! Terminal live-input retirement and fresh instrument power-on through
//! prepared callback publication, with independent wet and dry references.
use allocation_counter::measure;
use hex_audio::prepared;
use hex_engine::{
    PreparedRevision, SineTable,
    computer_keyboard::ComputerKeyboardInput,
    native_device_resources::{
        PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
    },
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};

fn prepare(
    project: &Project,
    store: &ResidentDeviceAssets,
) -> (Arc<PreparedRevision>, PreparedDeviceResources) {
    let cancel = AtomicBool::new(false);
    let resources =
        PreparedDeviceResources::prepare_on_control(store, project, 20_000_000, &cancel).unwrap();
    let revision = PreparedRevision::prepare_with_device_resources(
        project,
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
        &hex_engine::audio_transform::AudioTransformPreparation::default(),
        &resources,
        resources.compiler_scan_work_on_control(project).unwrap(),
        &cancel,
    )
    .unwrap();
    (Arc::new(revision), resources)
}

#[test]
fn live_source_power_cycle_and_new_generation_have_zero_callback_allocations() {
    for key_up in ["before-disable", "while-disabled", "after-reenable"] {
        for block in [1, 7, 64, 512] {
            check(key_up, block, false);
        }
    }
}

#[test]
fn loaded_sampler_host_trim_and_power_fade_have_zero_callback_allocations() {
    check("before-disable", 64, true);
}

fn check(key_up: &str, block: usize, host_output: bool) {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 32).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "memory/release.wav".into(),
            sample_rate: 8000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset");
    };
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let mut sampler = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
    sampler.mode = hex_devices::sampler::state::AuthoredMode::Classic {
        loop_enabled: true,
        loop_region: sampler.region,
    };
    sampler.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.1,
    };
    let state = sampler
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    editor
        .execute(Command::InsertInstrument { track, state })
        .unwrap();
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    if host_output {
        use hex_project::{
            DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, ModulationAmount,
            ModulationDestination, ModulationOutputId, ModulationRange, ModulationRouteId,
            ModulationSourceId, Modulator,
        };
        let next = editor.project().next_identity();
        let source = ModulationSourceId::new(next).unwrap();
        editor
            .execute(Command::ReplaceDeviceModulation {
                device,
                modulation: DeviceModulation::new(
                    vec![Modulator {
                        id: source,
                        enabled: true,
                        color: Default::default(),
                        state: hex_devices::modulation::macro_value::Settings { value: 1.0 }
                            .encode()
                            .unwrap(),
                    }],
                    vec![DeviceModulationRoute {
                        id: ModulationRouteId::new(next + 1).unwrap(),
                        source,
                        output: ModulationOutputId::MAIN,
                        destination: ModulationDestination::Device {
                            device_type: hex_devices::sampler::state::TYPE_ID,
                            target: DeviceAutomationTarget::HostOutputGain,
                            vst3_class: None,
                        },
                        amount: ModulationAmount::new(-6.0 / 84.0).unwrap(),
                        range: ModulationRange::default(),
                        enabled: true,
                    }],
                )
                .unwrap(),
            })
            .unwrap();
    }
    let Created::Track(unrelated) = editor
        .execute(Command::AddMidiTrack {
            name: "unrelated".into(),
        })
        .unwrap()
    else {
        panic!("unrelated");
    };
    editor
        .execute(Command::Gain {
            track: unrelated,
            gain: 0.25,
        })
        .unwrap();
    let Created::Clip(other_clip) = editor
        .execute(Command::PlaceMidiClip {
            track: unrelated,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: other_clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 67,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let mut store = ResidentDeviceAssets::default();
    store
        .insert_on_control(
            (
                asset,
                hex_engine::OwnedAudioSource::try_resident(
                    8000,
                    vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.25, 0.125]],
                )
                .ok()
                .unwrap(),
            ),
            0,
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 256,
                handle_rows: 64,
                root_rows: 32,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
            4,
            &AtomicBool::new(false),
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();
    let old = Arc::new(ComputerKeyboardInput::default());
    let fresh = Arc::new(ComputerKeyboardInput::default());
    let reference = Arc::new(ComputerKeyboardInput::default());
    for input in [&old, &reference] {
        input.set_key(0, Some((60, 127, 1))).unwrap();
    }
    let make = |project: &Project, input: Option<&Arc<ComputerKeyboardInput>>| {
        let (revision, resources) = prepare(project, &store);
        let mut playback =
            Playback::try_from_prepared_with_device_resources(revision, &resources).unwrap();
        if let Some(input) = input {
            playback
                .prepare_computer_keyboard(project, track, Arc::clone(input))
                .unwrap();
        }
        playback
    };
    let mut dry_editor = Editor::new(editor.project().clone(), 2).unwrap();
    dry_editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let mut dry = make(dry_editor.project(), None);
    dry.apply(PlaybackCommand::Play);
    let mut oracle = make(editor.project(), Some(&reference));
    oracle.apply(PlaybackCommand::Play);
    let (mut control, mut callback) = prepared::channel(8000).unwrap();
    control
        .configure_device_resource_publication_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 256,
                root_rows: 128,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
        )
        .unwrap();
    control
        .publish_with_policy_and_context_generation(
            make(editor.project(), Some(&old)),
            prepared::PublicationPolicy::InheritRuntime,
            19,
        )
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 512];
    let mut expected = actual;
    let mut position = 0;
    render(
        &mut callback,
        &mut oracle,
        &mut actual[..129],
        &mut expected[..129],
    );
    assert_eq!(&actual[..129], &expected[..129]);
    position += 129;
    assert!(
        actual[..129]
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.001)
    );
    if key_up == "before-disable" {
        old.set_key(0, None).unwrap();
        reference.set_key(0, None).unwrap();
        render(
            &mut callback,
            &mut oracle,
            &mut actual[..13],
            &mut expected[..13],
        );
        assert_eq!(&actual[..13], &expected[..13]);
        position += 13;
    }
    let mut dry_pcm = [[0.0; 2]; 512];
    dry.render(&mut dry_pcm[..position]).unwrap();

    const FADE: usize = 40;
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let off_revision = control
        .publish_with_policy_and_context_generation(
            make(editor.project(), None),
            prepared::PublicationPolicy::InheritRuntime,
            19,
        )
        .unwrap();
    for (chunk, offset) in (0..FADE + 24).step_by(block).enumerate() {
        let n = (FADE + 24 - offset).min(block);
        render(
            &mut callback,
            &mut oracle,
            &mut actual[..n],
            &mut expected[..n],
        );
        dry.render(&mut dry_pcm[..n]).unwrap();
        for i in 0..n {
            let gain = FADE.saturating_sub(offset + i) as f32 / FADE as f32;
            for channel in 0..2 {
                let dry = dry_pcm[i][channel];
                let target = dry + (expected[i][channel] - dry) * gain;
                assert!(
                    (actual[i][channel] - target).abs() < 0.000_001,
                    "{key_up} block={block} power-off frame={}",
                    offset + i
                );
            }
        }
        position += n;
        assert_eq!(control.status.recovery_position(), position as u64);
        assert_eq!(
            control
                .status
                .applied_revision
                .load(std::sync::atomic::Ordering::Acquire),
            off_revision
        );
        assert!(control.status.rejected_revision().is_none());
        if chunk == 0 {
            assert!(old.is_cancelled(), "power-off retires the old activation");
            if key_up == "while-disabled" {
                old.set_key(0, None).unwrap();
            }
            old.set_key(1, Some((67, 127, 2))).unwrap();
        }
    }
    control.reclaim();

    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: false,
        })
        .unwrap();
    let reference = Arc::new(ComputerKeyboardInput::default());
    let mut oracle = make(editor.project(), Some(&reference));
    oracle.try_inherit_runtime_from(&mut dry).unwrap();
    let on_revision = control
        .publish_with_policy_and_context_generation(
            make(editor.project(), Some(&fresh)),
            prepared::PublicationPolicy::InheritRuntime,
            19,
        )
        .unwrap();
    render(
        &mut callback,
        &mut oracle,
        &mut actual[..13],
        &mut expected[..13],
    );
    assert_eq!(&actual[..13], &expected[..13], "old key stays silent");
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        on_revision
    );
    assert!(control.status.rejected_revision().is_none());
    position += 13;
    control.reclaim();
    for input in [&fresh, &reference] {
        input.set_key(1, Some((67, 127, 1))).unwrap();
    }
    render(
        &mut callback,
        &mut oracle,
        &mut actual[..13],
        &mut expected[..13],
    );
    assert_eq!(&actual[..13], &expected[..13]);
    position += 13;
    old.set_key(0, None).unwrap();
    fresh.set_key(0, None).unwrap();
    render(
        &mut callback,
        &mut oracle,
        &mut actual[..13],
        &mut expected[..13],
    );
    assert_eq!(&actual[..13], &expected[..13]);
    position += 13;
    for input in [&fresh, &reference] {
        input.set_key(1, None).unwrap();
    }
    while position < 5000 {
        let n = (5000 - position).min(block);
        render(
            &mut callback,
            &mut oracle,
            &mut actual[..n],
            &mut expected[..n],
        );
        assert_eq!(
            &actual[..n],
            &expected[..n],
            "{key_up} block={block} frame={position}: release, future onset or unrelated source changed"
        );
        position += n;
    }
    assert_eq!(
        actual[0], [0.0; 2],
        "all notes end without a chased/stuck gate"
    );
    control.request_shutdown();
    zero(measure(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    }));
    control.reclaim();
}

fn render(
    callback: &mut prepared::Callback,
    oracle: &mut Playback,
    actual: &mut [[f32; 2]],
    expected: &mut [[f32; 2]],
) {
    oracle.render(expected).unwrap();
    zero(measure(|| {
        callback.begin_block();
        callback.render(actual).unwrap();
    }));
}
fn zero(info: allocation_counter::AllocationInfo) {
    assert_eq!(
        (
            info.count_total,
            info.bytes_total,
            info.count_current,
            info.bytes_current
        ),
        (0, 0, 0, 0)
    );
}
