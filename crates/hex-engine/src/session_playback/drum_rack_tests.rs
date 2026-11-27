use super::*;
use crate::native_device_resources::{
    PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
};
use hex_lattice::{
    DrumRackDefinition, DrumRackPadSpec, DrumRackSpec, NativeInstrumentDefinition,
    NativeInstrumentRegistry,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, NativeDeviceState, ParameterValue,
};
use hex_project::{BeatTime, Command, Created, Editor, Project, SourceId, TrackId};
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

fn subtractive_pad_state() -> NativeDeviceState {
    let base = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Instrument,
        hex_devices::subtractive::SUBTRACTIVE_TYPE_ID,
    )
    .unwrap();
    let attack = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        &base,
        hex_devices::subtractive::parameter::ATTACK,
        ParameterValue::Float(0.001),
    )
    .unwrap();
    crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        &attack,
        hex_devices::subtractive::parameter::RELEASE,
        ParameterValue::Float(0.002),
    )
    .unwrap()
}

fn sampler_pad_state(asset: hex_project::AssetId) -> NativeDeviceState {
    let mut state = hex_devices::sampler::state::SamplerState::for_source_frames(8).unwrap();
    state.mode = hex_devices::sampler::state::AuthoredMode::Classic {
        loop_enabled: true,
        loop_region: state.region,
    };
    state.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.0,
    };
    state
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap()
}

fn insert_asset_into_store(store: &mut ResidentDeviceAssets, asset: hex_project::AssetId) {
    store
        .insert_on_control(
            (
                asset,
                crate::OwnedAudioSource::try_resident(RATE, KICK_PCM.to_vec())
                    .ok()
                    .unwrap(),
            ),
            0,
            crate::AudioStorageLedgerReservation {
                data_rows: 256,
                handle_rows: 64,
                root_rows: 32,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
            64,
            &AtomicBool::new(false),
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();
}

fn prepare_with_drum_rack(
    project: &Project,
    store: &ResidentDeviceAssets,
) -> Result<(Arc<PreparedRevision>, PreparedDeviceResources), crate::Invalid> {
    let cancel = AtomicBool::new(false);
    let effects = hex_devices::native_registry().unwrap();
    let builtin = hex_devices::native_instrument_registry().unwrap();
    let mut defs: Vec<&dyn NativeInstrumentDefinition> = builtin.definitions().to_vec();
    let drum_def = DrumRackDefinition::new(&builtin).unwrap();
    defs.push(&drum_def);
    let instruments = NativeInstrumentRegistry::new(&defs).unwrap();
    let resources = PreparedDeviceResources::prepare_on_control_with_registry(
        store,
        project,
        &instruments,
        20_000_000,
        &cancel,
    )
    .map_err(|_| crate::Invalid("missing or invalid device resources"))?;
    let table = Arc::new(hex_devices::SineTable::new());
    let transforms = crate::audio_transform::AudioTransformPreparation::default();
    let revision = PreparedRevision::prepare_with_device_resources_and_registries(
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
    )?;
    Ok((Arc::new(revision), resources))
}

fn build_shared_drum_project(
    build_instrument: impl FnOnce(hex_project::AssetId) -> NativeDeviceState,
) -> (
    Editor,
    ResidentDeviceAssets,
    hex_project::AssetId,
    SourceId,
    TrackId,
    TrackId,
) {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
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
    let instrument = build_instrument(asset);
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "shared-drum-rack".into(),
            devices: vec![instrument],
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
        panic!("expected track a");
    };
    let Created::Track(track_b) = editor
        .execute(Command::AddMidiTrack {
            name: "snare-track".into(),
        })
        .unwrap()
    else {
        panic!("expected track b");
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
    assert_eq!(editor.project().native_sources().len(), 1);
    assert_eq!(
        editor.project().native_source(source).unwrap().receiver(),
        Some(track_a)
    );

    let mut store = ResidentDeviceAssets::default();
    insert_asset_into_store(&mut store, asset);
    (editor, store, asset, source, track_a, track_b)
}

fn render_blocks(
    prepared: (Arc<PreparedRevision>, PreparedDeviceResources),
    block_size: usize,
    blocks: usize,
) -> Vec<Frame> {
    let (rev, resources) = prepared;
    let mut pb = SessionPlayback::new_with_device_resources(rev, &resources).unwrap();
    pb.apply(SessionPlaybackCommand::Play).unwrap();
    let mut out = vec![[0.0; 2]; block_size * blocks];
    for chunk in out.chunks_mut(block_size) {
        pb.render(chunk).unwrap();
    }
    out
}

#[test]
fn two_pad_sampler_and_subtractive_drum_rack_matches_exact_summed_pcm_and_choke_cuts_sampler() {
    let (mut editor, store, asset, source, track_a, track_b) = build_shared_drum_project(|asset| {
        DrumRackSpec {
            rack_id: id(10),
            gain: 1.0,
            pads: vec![
                DrumRackPadSpec {
                    pad_id: id(1),
                    device_id: id(101),
                    name: "kick-sampler".into(),
                    receive_note: 36,
                    play_note: 60,
                    choke_group: None,
                    gain: 0.75,
                    muted: false,
                    state: Some(sampler_pad_state(asset)),
                },
                DrumRackPadSpec {
                    pad_id: id(2),
                    device_id: id(102),
                    name: "snare-synth".into(),
                    receive_note: 38,
                    play_note: 64,
                    choke_group: None,
                    gain: 0.5,
                    muted: false,
                    state: Some(subtractive_pad_state()),
                },
            ],
        }
        .encode_state()
        .unwrap()
    });

    // Track A plays pitch 36 (Sampler pad) at beat 0; Track B plays pitch 38 (Subtractive pad) at beat 1/64.
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
            duration: beat_frac(1, 8),
            pitch: 36,
            channel: 0,
            velocity: 110,
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
            velocity: 95,
        })
        .unwrap();

    // Oracle 1: isolated Sampler playing pitch 60 (play_note) on clip A only.
    let (mut oracle_a_ed, store_a, _, _, oa_track_a, _) =
        build_shared_drum_project(sampler_pad_state);
    let Created::Clip(oa_clip) = oracle_a_ed
        .execute(Command::PlaceMidiClip {
            track: oa_track_a,
            start: BeatTime::ZERO,
            length: beat(2),
        })
        .unwrap()
    else {
        panic!("oa clip");
    };
    oracle_a_ed
        .execute(Command::AddMidiNote {
            clip: oa_clip,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 8),
            pitch: 60,
            channel: 0,
            velocity: 110,
        })
        .unwrap();

    // Oracle 2: isolated Subtractive playing pitch 64 (play_note) on clip B only.
    let (mut oracle_b_ed, store_b, _, _, _, ob_track_b) =
        build_shared_drum_project(|_| subtractive_pad_state());
    let Created::Clip(ob_clip) = oracle_b_ed
        .execute(Command::PlaceMidiClip {
            track: ob_track_b,
            start: BeatTime::ZERO,
            length: beat(2),
        })
        .unwrap()
    else {
        panic!("ob clip");
    };
    oracle_b_ed
        .execute(Command::AddMidiNote {
            clip: ob_clip,
            start: beat_frac(1, 64),
            duration: beat_frac(1, 32),
            pitch: 64,
            channel: 0,
            velocity: 95,
        })
        .unwrap();

    let drum_pcm = render_blocks(
        prepare_with_drum_rack(editor.project(), &store).unwrap(),
        128,
        16,
    );
    let oracle_a = render_blocks(
        prepare_with_drum_rack(oracle_a_ed.project(), &store_a).unwrap(),
        128,
        16,
    );
    let oracle_b = render_blocks(
        prepare_with_drum_rack(oracle_b_ed.project(), &store_b).unwrap(),
        128,
        16,
    );

    let mut max_err = 0.0_f32;
    let mut energy_a = 0.0_f32;
    let mut energy_b = 0.0_f32;
    for ((d, a), b) in drum_pcm.iter().zip(&oracle_a).zip(&oracle_b) {
        energy_a += a[0].abs() + a[1].abs();
        energy_b += b[0].abs() + b[1].abs();
        let expected_l = a[0] * 0.75 + b[0] * 0.5;
        let expected_r = a[1] * 0.75 + b[1] * 0.5;
        max_err = max_err
            .max((d[0] - expected_l).abs())
            .max((d[1] - expected_r).abs());
    }
    assert!(energy_a > 1.0, "sampler pad must produce audio");
    assert!(energy_b > 1.0, "subtractive pad must produce audio");
    assert!(
        max_err < 1e-5,
        "drum rack must match exact weighted sum of independent pads, max_err={max_err}"
    );

    // Missing asset in store refuses preparation cleanly.
    let empty_store = ResidentDeviceAssets::default();
    assert!(prepare_with_drum_rack(editor.project(), &empty_store).is_err());

    // Now put both pads in choke group 1: snare at beat 1/64 cuts Sampler loop immediately!
    let choked_spec = DrumRackSpec {
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
                state: Some(sampler_pad_state(asset)),
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
                state: Some(subtractive_pad_state()),
            },
        ],
    };
    let source_device = editor
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device: source_device,
            state: choked_spec.encode_state().unwrap(),
        })
        .unwrap();
    let choked_pcm = render_blocks(
        prepare_with_drum_rack(editor.project(), &store).unwrap(),
        128,
        16,
    );
    // At beat 1/64 (frame 375 at 120 BPM, 48 kHz), snare triggers and chokes Sampler
    // (which finishes its anti-click release floor well before frame 900, while unchoked
    // Sampler would keep looping until frame 3000!).
    let unchoked_tail_energy: f32 = oracle_a[900..1500]
        .iter()
        .map(|f| f[0].abs() + f[1].abs())
        .sum();
    assert!(unchoked_tail_energy > 10.0);
    let mut post_choke_err = 0.0_f32;
    for idx in 900..1500 {
        post_choke_err = post_choke_err
            .max((choked_pcm[idx][0] - oracle_b[idx][0] * 0.5).abs())
            .max((choked_pcm[idx][1] - oracle_b[idx][1] * 0.5).abs());
    }
    assert!(
        post_choke_err < 1e-5,
        "choke group 1 must silence Sampler pad cleanly, err={post_choke_err}"
    );
}

#[test]
fn live_drum_rack_pad_rename_remap_removal_undo_redo_and_snapshot_roundtrip() {
    let (mut editor, store, asset, source, track_a, _track_b) =
        build_shared_drum_project(|asset| {
            DrumRackSpec {
                rack_id: id(20),
                gain: 1.0,
                pads: vec![
                    DrumRackPadSpec {
                        pad_id: id(1),
                        device_id: id(201),
                        name: "kick".into(),
                        receive_note: 36,
                        play_note: 60,
                        choke_group: None,
                        gain: 1.0,
                        muted: false,
                        state: Some(sampler_pad_state(asset)),
                    },
                    DrumRackPadSpec {
                        pad_id: id(2),
                        device_id: id(202),
                        name: "snare".into(),
                        receive_note: 38,
                        play_note: 64,
                        choke_group: None,
                        gain: 0.5,
                        muted: false,
                        state: Some(subtractive_pad_state()),
                    },
                ],
            }
            .encode_state()
            .unwrap()
        });

    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track: track_a,
            start: BeatTime::ZERO,
            length: beat(2),
        })
        .unwrap()
    else {
        panic!("clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 16),
            pitch: 36,
            channel: 0,
            velocity: 115,
        })
        .unwrap();

    // Start playback so kick Sampler pad is actively sounding.
    let (rev_a, res_a) = prepare_with_drum_rack(editor.project(), &store).unwrap();
    let mut pb_a = SessionPlayback::new_with_device_resources(Arc::clone(&rev_a), &res_a).unwrap();
    pb_a.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0_f32; 2]; 128];
    pb_a.render(&mut first).unwrap();
    assert!(first.iter().any(|f| f[0].abs() > 1e-4));

    // Rename pad 1, remap 36 -> 40, and remove idle pad 2 via canonical Command.
    let source_device = editor
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    let mut edited = DrumRackSpec::decode_and_migrate_state(
        editor
            .project()
            .native_source(source)
            .unwrap()
            .instrument()
            .state(),
    )
    .unwrap();
    edited.rename_pad(id(1), "808-kick").unwrap();
    edited.remap_pad(id(1), 40, 60).unwrap();
    edited.remove_pad(id(2)).unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: source_device,
            state: edited.encode_state().unwrap(),
        })
        .unwrap();

    // Live handoff inherits active Sampler voice cleanly via DrumRackRetargetProof!
    let (rev_b, res_b) = prepare_with_drum_rack(editor.project(), &store).unwrap();
    let mut pb_b = SessionPlayback::new_with_device_resources(Arc::clone(&rev_b), &res_b).unwrap();
    pb_b.try_inherit_state_from(&mut pb_a).unwrap();

    // Render past note-off (beat 1/16 = 1500 frames) and assert zero stuck notes!
    let mut tail = [[0.0_f32; 2]; 256];
    for _ in 0..8 {
        pb_b.render(&mut tail).unwrap();
    }
    assert!(tail.iter().all(|f| f[0].abs() < 1e-6 && f[1].abs() < 1e-6));

    // Snapshot roundtrip preserves edited Drum Rack & outer asset reference.
    let snap = editor.project().snapshot();
    let restored = Project::from_snapshot(snap).unwrap();
    let restored_dev = restored.native_source(source).unwrap().instrument().state();
    let restored_spec = DrumRackSpec::decode_and_migrate_state(restored_dev).unwrap();
    assert_eq!(restored_spec.pads.len(), 1);
    assert_eq!(restored_spec.pads[0].name, "808-kick");
    assert_eq!(restored_spec.pads[0].receive_note, 40);
    assert_eq!(restored_dev.asset_references().len(), 1);
    assert_eq!(
        restored_dev.asset_references()[0].asset.value(),
        asset.value()
    );

    // Canonical Undo/Redo restores pad 2 and original receive_note 36.
    assert!(editor.undo());
    let undone = DrumRackSpec::decode_and_migrate_state(
        editor
            .project()
            .native_source(source)
            .unwrap()
            .instrument()
            .state(),
    )
    .unwrap();
    assert_eq!(undone.pads.len(), 2);
    assert_eq!(undone.pads[0].receive_note, 36);

    assert!(editor.redo());
    let redone = DrumRackSpec::decode_and_migrate_state(
        editor
            .project()
            .native_source(source)
            .unwrap()
            .instrument()
            .state(),
    )
    .unwrap();
    assert_eq!(redone.pads.len(), 1);
    assert_eq!(redone.pads[0].receive_note, 40);

    // Stop transitions cleanly to silence.
    pb_b.apply(SessionPlaybackCommand::Stop).unwrap();
    let mut stopped = [[0.0_f32; 2]; 128];
    pb_b.render(&mut stopped).unwrap();
    assert!(stopped.iter().all(|f| f[0] == 0.0 && f[1] == 0.0));
}
