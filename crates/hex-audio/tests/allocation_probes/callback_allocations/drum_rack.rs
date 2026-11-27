use allocation_counter::measure;
use hex_engine::{
    AudioStorageLedgerReservation, Frame, NativeDeviceClass, OwnedAudioSource, PreparedRevision,
    SessionPlayback, SessionPlaybackCommand, SineTable,
    audio_transform::AudioTransformPreparation,
    default_native_device_state_for_class,
    native_device_resources::{
        PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
    },
    patch_native_device_parameter_for_class,
};
use hex_lattice::{DrumRackDefinition, DrumRackPadSpec, DrumRackSpec};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    num::{NonZeroU8, NonZeroU64},
    sync::{Arc, atomic::AtomicBool},
};

const RATE: u32 = 48_000;
const KICK_PCM: [Frame; 8] = [
    [0.5, 0.5],
    [0.25, 0.25],
    [-0.25, -0.25],
    [-0.5, -0.5],
    [0.375, 0.375],
    [0.125, 0.125],
    [-0.125, -0.125],
    [-0.375, -0.375],
];

fn id(v: u64) -> NonZeroU64 {
    NonZeroU64::new(v).unwrap()
}

fn beat(quarter_notes: u64) -> BeatTime {
    BeatTime::from_raw(quarter_notes << 32)
}

fn beat_frac(num: u64, den: u64) -> BeatTime {
    BeatTime::from_raw((num << 32) / den)
}

#[test]
fn two_pad_sampler_and_subtractive_drum_rack_allocates_nothing_on_callback() {
    let sub_base = default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        hex_devices::subtractive::SUBTRACTIVE_TYPE_ID,
    )
    .unwrap();
    let sub_attack = patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &sub_base,
        hex_devices::subtractive::parameter::ATTACK,
        ParameterValue::Float(0.001),
    )
    .unwrap();
    let sub = patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &sub_attack,
        hex_devices::subtractive::parameter::RELEASE,
        ParameterValue::Float(0.002),
    )
    .unwrap();

    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "kick.wav".into(),
            sample_rate: RATE,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("expected asset");
    };

    let mut sampler = hex_devices::sampler::state::SamplerState::for_source_frames(8).unwrap();
    sampler.mode = hex_devices::sampler::state::AuthoredMode::Classic {
        loop_enabled: true,
        loop_region: sampler.region,
    };
    sampler.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.0,
    };
    let sampler_state = sampler
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();

    let spec = DrumRackSpec {
        rack_id: id(10),
        gain: 1.0,
        pads: vec![
            DrumRackPadSpec {
                pad_id: id(1),
                device_id: id(101),
                name: "kick-sampler".into(),
                receive_note: 36,
                play_note: 60,
                choke_group: NonZeroU8::new(1),
                gain: 0.75,
                muted: false,
                output_port: hex_lattice::NativeOutputPortId::MAIN_STEREO,
                state: Some(sampler_state),
            },
            DrumRackPadSpec {
                pad_id: id(2),
                device_id: id(102),
                name: "snare-synth".into(),
                receive_note: 38,
                play_note: 64,
                choke_group: NonZeroU8::new(1),
                gain: 0.5,
                muted: false,
                output_port: hex_lattice::NativeOutputPortId::MAIN_STEREO,
                state: Some(sub),
            },
        ],
    };

    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "drum-rack-probe".into(),
            devices: vec![spec.encode_state().unwrap()],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("expected native source");
    };
    let Created::Track(track_a) = editor
        .execute(Command::AddMidiTrack {
            name: "kick-track".into(),
        })
        .unwrap()
    else {
        panic!("track a");
    };
    let Created::Track(track_b) = editor
        .execute(Command::AddMidiTrack {
            name: "snare-track".into(),
        })
        .unwrap()
    else {
        panic!("track b");
    };
    for track in [track_a, track_b] {
        if editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .is_some()
        {
            editor.execute(Command::RemoveInstrument { track }).unwrap();
        }
        editor
            .execute(Command::BindTrackSource {
                track,
                source: Some(source),
            })
            .unwrap();
    }

    let Created::Clip(clip_a) = editor
        .execute(Command::PlaceMidiClip {
            track: track_a,
            start: BeatTime::ZERO,
            length: beat(2),
        })
        .unwrap()
    else {
        panic!("clip a");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: clip_a,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 32),
            pitch: 36,
            channel: 0,
            velocity: 115,
        })
        .unwrap();
    let Created::Clip(clip_b) = editor
        .execute(Command::PlaceMidiClip {
            track: track_b,
            start: BeatTime::ZERO,
            length: beat(2),
        })
        .unwrap()
    else {
        panic!("clip b");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: clip_b,
            start: beat_frac(1, 64),
            duration: beat_frac(1, 32),
            pitch: 38,
            channel: 0,
            velocity: 100,
        })
        .unwrap();

    // Control-side preparation & lease admission strictly outside callback measurement.
    let cancel = AtomicBool::new(false);
    let mut store = ResidentDeviceAssets::default();
    store
        .insert_on_control(
            (
                asset,
                OwnedAudioSource::try_resident(RATE, KICK_PCM.to_vec())
                    .ok()
                    .unwrap(),
            ),
            0,
            AudioStorageLedgerReservation {
                data_rows: 256,
                handle_rows: 64,
                root_rows: 32,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
            64,
            &cancel,
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();

    let prepare = |project: &Project| {
        let effects = hex_devices::native_registry().unwrap();
        let instruments = hex_devices::native_instrument_registry().unwrap();
        let resources = PreparedDeviceResources::prepare_on_control_with_registry(
            &store,
            project,
            &instruments,
            20_000_000,
            &cancel,
        )
        .unwrap();
        let table = Arc::new(SineTable::new());
        let transforms = AudioTransformPreparation::default();
        let revision = Arc::new(
            PreparedRevision::prepare_with_device_resources_and_registries(
                project,
                &BTreeMap::new(),
                table,
                None,
                &transforms,
                &resources,
                20_000_000,
                &cancel,
                &effects,
                &instruments,
                None,
            )
            .unwrap(),
        );
        (revision, resources)
    };

    let (rev_a, res_a) = prepare(editor.project());
    let mut pb_a = SessionPlayback::new_with_device_resources(rev_a, &res_a).unwrap();

    // Prepare a live-remapped revision on control for retarget measurement.
    let source_device = editor
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    let mut spec_b = spec;
    spec_b.rename_pad(id(1), "kick-remapped").unwrap();
    spec_b.remap_pad(id(1), 40, 60).unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: source_device,
            state: spec_b.encode_state().unwrap(),
        })
        .unwrap();
    let (rev_b, res_b) = prepare(editor.project());
    let mut pb_b = SessionPlayback::new_with_device_resources(rev_b, &res_b).unwrap();

    let mut scratch = vec![[0.0_f32; 2]; 256];
    let info = measure(|| {
        pb_a.apply(SessionPlaybackCommand::Play).unwrap();
        // Renders Sampler onset + Subtractive choke cut of Sampler!
        for _ in 0..4 {
            pb_a.render(&mut scratch).unwrap();
        }
        // Live retarget handoff on callback scope.
        pb_b.try_inherit_state_from(&mut pb_a).unwrap();
        for _ in 0..4 {
            pb_b.render(&mut scratch).unwrap();
        }
        pb_b.apply(SessionPlaybackCommand::Stop).unwrap();
        pb_b.render(&mut scratch).unwrap();
    });
    assert_eq!(
        (
            info.count_total,
            info.bytes_total,
            info.count_current,
            info.bytes_current
        ),
        (0, 0, 0, 0),
        "drum rack callback (Sampler+Subtractive, choke cut, live retarget, Stop) must allocate nothing: {info:?}"
    );
    // Drop retired playback & leased resources off callback on control.
    drop(pb_a);
    drop(pb_b);
    drop(store);
}

#[test]
fn two_pad_multi_output_drum_rack_sampler_and_subtractive_allocates_nothing_on_callback() {
    use hex_lattice::{
        DevicePreparationContext, MultiOutputDestinationBuffer, MultiOutputInstrumentRuntime,
        NativeInstrumentEvent, NativeMultiOutputLimits, NativeMultiOutputPlan,
        NativeMultiOutputRoute, NativeMultiOutputTopology, NativeOutputDestination,
        NativeOutputDestinationInfo, NativeOutputPortId, PreparedMultiOutputRouter, TuningTable,
    };

    let sub_base = default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        hex_devices::subtractive::SUBTRACTIVE_TYPE_ID,
    )
    .unwrap();
    let sub = patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &sub_base,
        hex_devices::subtractive::parameter::RELEASE,
        ParameterValue::Float(0.002),
    )
    .unwrap();

    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "kick-mo.wav".into(),
            sample_rate: RATE,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("asset");
    };
    let mut sampler = hex_devices::sampler::state::SamplerState::for_source_frames(8).unwrap();
    sampler.mode = hex_devices::sampler::state::AuthoredMode::Classic {
        loop_enabled: true,
        loop_region: sampler.region,
    };
    sampler.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.0,
    };
    let sampler_state = sampler
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();

    let spec = DrumRackSpec {
        rack_id: id(40),
        gain: 1.0,
        pads: vec![
            DrumRackPadSpec {
                pad_id: id(1),
                device_id: id(401),
                name: "kick-main".into(),
                receive_note: 36,
                play_note: 60,
                choke_group: NonZeroU8::new(1),
                gain: 0.8,
                muted: false,
                output_port: NativeOutputPortId::MAIN_STEREO,
                state: Some(sampler_state),
            },
            DrumRackPadSpec {
                pad_id: id(2),
                device_id: id(402),
                name: "snare-aux".into(),
                receive_note: 38,
                play_note: 64,
                choke_group: NonZeroU8::new(1),
                gain: 0.6,
                muted: false,
                output_port: NativeOutputPortId::AUX_STEREO,
                state: Some(sub),
            },
        ],
    };
    let encoded = spec.encode_state().unwrap();
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "drum-mo-probe".into(),
            devices: vec![encoded.clone()],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source");
    };
    let Created::Track(track_a) = editor
        .execute(Command::AddMidiTrack {
            name: "receiver".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    if editor
        .project()
        .track(track_a)
        .unwrap()
        .instrument()
        .is_some()
    {
        editor
            .execute(Command::RemoveInstrument { track: track_a })
            .unwrap();
    }
    editor
        .execute(Command::BindTrackSource {
            track: track_a,
            source: Some(source),
        })
        .unwrap();

    // Control-side preparation & lease admission outside callback measurement.
    let cancel = AtomicBool::new(false);
    let mut store = ResidentDeviceAssets::default();
    store
        .insert_on_control(
            (
                asset,
                OwnedAudioSource::try_resident(RATE, KICK_PCM.to_vec())
                    .ok()
                    .unwrap(),
            ),
            0,
            AudioStorageLedgerReservation {
                data_rows: 256,
                handle_rows: 64,
                root_rows: 32,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
            64,
            &cancel,
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();
    let leaf = hex_devices::native_leaf_instrument_registry().unwrap();
    let drum_def = DrumRackDefinition::new(leaf).unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let resources = PreparedDeviceResources::prepare_on_control_with_registry(
        &store,
        editor.project(),
        &instruments,
        20_000_000,
        &cancel,
    )
    .unwrap();
    let views = [hex_lattice::DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: hex_lattice::DeviceResourceIdentity {
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
            content: [7_u8; 32],
            derivation_version: 1,
            derivation: [1_u8; 32],
        },
        sample_rate: RATE,
        pcm: hex_lattice::DeviceResourcePcm::Stereo(&KICK_PCM),
    }];
    let borrowed_resources = hex_lattice::BorrowedDeviceResources(&views);
    let tuning = TuningTable::default();
    let ctx = DevicePreparationContext {
        sample_rate: RATE,
        tuning: &tuning,
        resources: &borrowed_resources,
    };

    let prepared = drum_def
        .prepare_multi_output_runtime(&encoded, &ctx, None)
        .unwrap();
    let ports = prepared.output_port_descriptors();
    let mut runtime = prepared.create_runtime();

    let source_nz = id(source.value());
    let topology = NativeMultiOutputTopology {
        source_host: NativeOutputDestination::Track(id(1)),
        destinations: vec![
            NativeOutputDestinationInfo {
                destination: NativeOutputDestination::Track(id(2)),
                latency_frames: 4,
            },
            NativeOutputDestinationInfo {
                destination: NativeOutputDestination::Return(id(10)),
                latency_frames: 0,
            },
        ],
        edges: Vec::new(),
    };
    let plan = NativeMultiOutputPlan {
        source_id: source_nz,
        routes: vec![
            NativeMultiOutputRoute {
                route_id: id(1),
                source_id: source_nz,
                port_id: NativeOutputPortId::MAIN_STEREO,
                destination: NativeOutputDestination::Track(id(2)),
                gain: 0.9,
                enabled: true,
            },
            NativeMultiOutputRoute {
                route_id: id(2),
                source_id: source_nz,
                port_id: NativeOutputPortId::AUX_STEREO,
                destination: NativeOutputDestination::Return(id(10)),
                gain: 0.7,
                enabled: true,
            },
        ],
    };
    let limits = NativeMultiOutputLimits {
        max_block_frames: 256,
        ..NativeMultiOutputLimits::default()
    };
    let mut router_a =
        PreparedMultiOutputRouter::prepare_on_control(&plan, ports, &topology, limits).unwrap();
    let mut router_b =
        PreparedMultiOutputRouter::prepare_on_control(&plan, ports, &topology, limits).unwrap();

    let mut port_main = vec![[0.0_f32; 2]; 256];
    let mut port_aux = vec![[0.0_f32; 2]; 256];
    let mut dest_track = vec![[0.0_f32; 2]; 256];
    let mut dest_return = vec![[0.0_f32; 2]; 256];
    let events_1 = [
        NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 36,
            velocity: 115,
        },
        NativeInstrumentEvent::NoteOn {
            frame_offset: 64,
            occurrence_id: 2,
            pitch: 38,
            velocity: 100,
        },
    ];
    let events_2 = [
        NativeInstrumentEvent::NoteOff {
            frame_offset: 32,
            occurrence_id: 1,
        },
        NativeInstrumentEvent::NoteOff {
            frame_offset: 64,
            occurrence_id: 2,
        },
    ];

    let info = measure(|| {
        MultiOutputInstrumentRuntime::preflight_events(
            &runtime,
            &events_1,
            256,
            &borrowed_resources,
        )
        .unwrap();
        runtime
            .process_block_multi_output(
                &events_1,
                &mut [&mut port_main, &mut port_aux],
                None,
                &borrowed_resources,
            )
            .unwrap();
        router_a.begin_block();
        router_a
            .route_ports(
                &[&port_main, &port_aux],
                &mut [
                    MultiOutputDestinationBuffer {
                        destination: NativeOutputDestination::Track(id(2)),
                        frames: &mut dest_track,
                    },
                    MultiOutputDestinationBuffer {
                        destination: NativeOutputDestination::Return(id(10)),
                        frames: &mut dest_return,
                    },
                ],
            )
            .unwrap();
        router_a.commit_block();

        // Compatible route handoff + second block + Stop/reset on callback.
        let inherited = router_b.inherit_compatible_routes_from(&router_a).unwrap();
        assert_eq!(inherited, 2);
        runtime
            .process_block_multi_output(
                &events_2,
                &mut [&mut port_main, &mut port_aux],
                None,
                &borrowed_resources,
            )
            .unwrap();
        router_b.begin_block();
        router_b
            .route_ports(
                &[&port_main, &port_aux],
                &mut [
                    MultiOutputDestinationBuffer {
                        destination: NativeOutputDestination::Track(id(2)),
                        frames: &mut dest_track,
                    },
                    MultiOutputDestinationBuffer {
                        destination: NativeOutputDestination::Return(id(10)),
                        frames: &mut dest_return,
                    },
                ],
            )
            .unwrap();
        router_b.commit_block();

        MultiOutputInstrumentRuntime::reset(&mut runtime);
        router_b.reset();
    });
    assert_eq!(
        (
            info.count_total,
            info.bytes_total,
            info.count_current,
            info.bytes_current
        ),
        (0, 0, 0, 0),
        "multi-output drum rack callback (Sampler+Subtractive, PDC, choke, handoff, Stop) must allocate nothing: {info:?}"
    );
    // Drop retired runtime, routers, and leased resources off-callback on control.
    drop(runtime);
    drop(router_a);
    drop(router_b);
    drop(resources);
    drop(store);
}
