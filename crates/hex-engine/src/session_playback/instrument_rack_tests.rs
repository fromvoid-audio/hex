use super::*;
use hex_lattice::{
    InstrumentRackBranchSpec, InstrumentRackDefinition, InstrumentRackSpec,
    NativeInstrumentDefinition, NativeInstrumentRegistry,
};
use hex_lattice_contract::{NativeDeviceState, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project, SourceId, TrackId};
use std::{collections::BTreeMap, num::NonZeroU64};

const RATE: u32 = 48_000;

fn id(v: u64) -> NonZeroU64 {
    NonZeroU64::new(v).unwrap()
}

fn beat(quarter_notes: u64) -> BeatTime {
    BeatTime::from_raw(quarter_notes << 32)
}

fn beat_frac(num: u64, den: u64) -> BeatTime {
    BeatTime::from_raw((num << 32) / den)
}

fn subtractive_state() -> NativeDeviceState {
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
        ParameterValue::Float(0.005),
    )
    .unwrap()
}

fn fm4_state() -> NativeDeviceState {
    crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Instrument,
        hex_devices::fm4::device::FM4_TYPE_ID,
    )
    .unwrap()
}

fn prepare_with_rack(project: &Project) -> Result<Arc<PreparedRevision>, crate::Invalid> {
    let builtin = hex_devices::native_instrument_registry().unwrap();
    let mut defs: Vec<&dyn NativeInstrumentDefinition> = builtin.definitions().to_vec();
    let rack_def = InstrumentRackDefinition::new(builtin).unwrap();
    defs.push(&rack_def);
    let combined = NativeInstrumentRegistry::new(&defs).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    PreparedRevision::prepare_with_instrument_registry(
        project,
        &BTreeMap::new(),
        table,
        None,
        &combined,
    )
    .map(Arc::new)
}

fn build_shared_source_project(
    source_instrument: NativeDeviceState,
) -> (Editor, SourceId, TrackId, TrackId) {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "shared-rack".into(),
            devices: vec![source_instrument],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("expected native source");
    };
    let Created::Track(track_a) = editor
        .execute(Command::AddMidiTrack {
            name: "contributor-a".into(),
        })
        .unwrap()
    else {
        panic!("expected track a");
    };
    let Created::Track(track_b) = editor
        .execute(Command::AddMidiTrack {
            name: "contributor-b".into(),
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
    // Single retained native source shared by two contributor tracks; track_a is the single receiver.
    assert_eq!(editor.project().native_sources().len(), 1);
    assert_eq!(
        editor.project().native_source(source).unwrap().receiver(),
        Some(track_a)
    );

    // Contributor A plays C4 (60) at beat 0; Contributor B plays G4 (67) at beat 1/64.
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
            pitch: 60,
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
            pitch: 67,
            channel: 0,
            velocity: 95,
        })
        .unwrap();

    (editor, source, track_a, track_b)
}

#[test]
fn two_branch_shared_source_rack_matches_exact_summed_pcm_bypass_removal_history_and_snapshot() {
    let sub = subtractive_state();
    let fm4 = fm4_state();

    let rack_spec = InstrumentRackSpec {
        rack_id: id(10),
        overlap_policy: hex_lattice::InstrumentRackOverlapPolicy::Layer,
        branches: [
            InstrumentRackBranchSpec {
                branch_id: id(1),
                device_id: id(101),
                gain: 0.6,
                bypassed: false,
                zone: hex_lattice::InstrumentRackZone::default(),
                state: Some(sub.clone()),
            },
            InstrumentRackBranchSpec {
                branch_id: id(2),
                device_id: id(102),
                gain: 0.4,
                bypassed: false,
                zone: hex_lattice::InstrumentRackZone::default(),
                state: Some(fm4.clone()),
            },
        ],
    };
    let rack_state = rack_spec.encode_state().unwrap();

    // Independent single-branch references sharing the exact same two contributors.
    let (sub_only_editor, _, _, _) = build_shared_source_project(sub.clone());
    let (fm4_only_editor, _, _, _) = build_shared_source_project(fm4.clone());
    let (mut rack_editor, source_id, _, _) = build_shared_source_project(rack_state.clone());

    let sub_rev = prepare_with_rack(sub_only_editor.project()).unwrap();
    let fm4_rev = prepare_with_rack(fm4_only_editor.project()).unwrap();
    let rack_rev = prepare_with_rack(rack_editor.project()).unwrap();

    let total_frames = 2_048_usize;
    let mut sub_out = vec![[0.0_f32; 2]; total_frames];
    let mut fm4_out = vec![[0.0_f32; 2]; total_frames];
    let mut rack_out = vec![[0.0_f32; 2]; total_frames];

    let mut sub_pb = SessionPlayback::new(sub_rev).unwrap();
    sub_pb.apply(SessionPlaybackCommand::Play).unwrap();
    sub_pb.render(&mut sub_out).unwrap();

    let mut fm4_pb = SessionPlayback::new(fm4_rev).unwrap();
    fm4_pb.apply(SessionPlaybackCommand::Play).unwrap();
    fm4_pb.render(&mut fm4_out).unwrap();

    let expected_sum: Vec<[f32; 2]> = sub_out
        .iter()
        .zip(&fm4_out)
        .map(|(a, b)| [a[0] * 0.6 + b[0] * 0.4, a[1] * 0.6 + b[1] * 0.4])
        .collect();
    assert!(sub_out.iter().flatten().any(|s| s.abs() > 1e-3));
    assert!(fm4_out.iter().flatten().any(|s| s.abs() > 1e-3));
    assert_ne!(sub_out, fm4_out);

    // 1. Shared-source two-branch rack matches weighted sum of both independent instruments across irregular partitions.
    let mut rack_pb = SessionPlayback::new(Arc::clone(&rack_rev)).unwrap();
    rack_pb.apply(SessionPlaybackCommand::Play).unwrap();
    let pattern = [1, 7, 31, 64, 128, 256];
    let mut cursor = 0_usize;
    let mut step = 0_usize;
    while cursor < total_frames {
        let len = pattern[step % pattern.len()].min(total_frames - cursor);
        rack_pb.render(&mut rack_out[cursor..cursor + len]).unwrap();
        cursor += len;
        step += 1;
    }
    for (frame_idx, (actual, expected)) in rack_out.iter().zip(&expected_sum).enumerate() {
        assert!(
            (actual[0] - expected[0]).abs() < 1e-6 && (actual[1] - expected[1]).abs() < 1e-6,
            "mismatch at frame {frame_idx}: actual={actual:?} expected={expected:?}"
        );
    }

    // 2. Stop + Seek(0) + Play reproduces bit-identically with zero stuck notes.
    rack_pb.apply(SessionPlaybackCommand::Stop).unwrap();
    assert!(
        rack_pb.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
    rack_pb.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    rack_pb.apply(SessionPlaybackCommand::Play).unwrap();
    let mut replayed = vec![[0.0_f32; 2]; total_frames];
    rack_pb.render(&mut replayed).unwrap();
    assert_eq!(replayed, rack_out);

    // 3. Snapshot save/reopen round-trip preserves retained NativeSource rack state and PCM.
    let reopened = Project::from_snapshot(rack_editor.project().snapshot()).unwrap();
    assert_eq!(reopened, *rack_editor.project());
    let reopened_rev = prepare_with_rack(&reopened).unwrap();
    let mut reopened_pb = SessionPlayback::new(reopened_rev).unwrap();
    reopened_pb.apply(SessionPlaybackCommand::Play).unwrap();
    let mut reopened_out = vec![[0.0_f32; 2]; total_frames];
    reopened_pb.render(&mut reopened_out).unwrap();
    assert_eq!(reopened_out, rack_out);

    // 4. Branch bypass and branch removal preserve sibling branch PCM + one-step Editor Undo/Redo.
    let source_device = rack_editor
        .project()
        .native_source(source_id)
        .unwrap()
        .instrument()
        .id();
    let mut bypassed_spec = rack_spec.clone();
    bypassed_spec.branches[1].bypassed = true;
    rack_editor
        .execute(Command::ReplaceDeviceState {
            device: source_device,
            state: bypassed_spec.encode_state().unwrap(),
        })
        .unwrap();
    let bypassed_rev = prepare_with_rack(rack_editor.project()).unwrap();
    let mut bypassed_pb = SessionPlayback::new(bypassed_rev).unwrap();
    bypassed_pb.apply(SessionPlaybackCommand::Play).unwrap();
    let mut bypassed_out = vec![[0.0_f32; 2]; total_frames];
    bypassed_pb.render(&mut bypassed_out).unwrap();
    for (actual, sub_s) in bypassed_out.iter().zip(&sub_out) {
        assert!((actual[0] - sub_s[0] * 0.6).abs() < 1e-6);
        assert!((actual[1] - sub_s[1] * 0.6).abs() < 1e-6);
    }

    // Remove Branch 1 (`state = None`) -> still exact 0.6 * sub_out.
    let mut removed_spec = rack_spec.clone();
    removed_spec.branches[1].state = None;
    rack_editor
        .execute(Command::ReplaceDeviceState {
            device: source_device,
            state: removed_spec.encode_state().unwrap(),
        })
        .unwrap();
    let removed_rev = prepare_with_rack(rack_editor.project()).unwrap();
    let mut removed_pb = SessionPlayback::new(removed_rev).unwrap();
    removed_pb.apply(SessionPlaybackCommand::Play).unwrap();
    let mut removed_out = vec![[0.0_f32; 2]; total_frames];
    removed_pb.render(&mut removed_out).unwrap();
    assert_eq!(removed_out, bypassed_out);

    // Editor Undo twice restores original two-branch rack state; Redo reapplies.
    assert!(rack_editor.undo());
    assert!(rack_editor.undo());
    assert_eq!(
        rack_editor
            .project()
            .native_source(source_id)
            .unwrap()
            .instrument()
            .state(),
        &rack_state
    );
    assert!(rack_editor.redo());

    // 5. Failed preparation (unknown child device type) refuses cleanly without touching active playback.
    let mut bad_spec = rack_spec;
    bad_spec.branches[1].state = Some(
        NativeDeviceState::new(
            hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.missing"),
            1,
            vec![],
        )
        .unwrap(),
    );
    let (bad_editor, _, _, _) = build_shared_source_project(bad_spec.encode_state().unwrap());
    assert!(prepare_with_rack(bad_editor.project()).is_err());
}

#[test]
fn instrument_rack_key_velocity_zones_crossfade_overlap_live_edit_and_migration_pcm_oracles() {
    use hex_lattice::{InstrumentRackOverlapPolicy, InstrumentRackZone};

    let sub = subtractive_state();
    let fm4 = fm4_state();

    // Shared-source project has:
    //   Contributor A: pitch 60, vel 110 at beat 0
    //   Contributor B: pitch 67, vel 95 at beat 1/64
    // Split zones:
    //   Branch 0 (Subtractive): keys 0..=63 (captures ONLY pitch 60!)
    //   Branch 1 (FM4):         keys 64..=127 (captures ONLY pitch 67!)
    let split_spec = InstrumentRackSpec {
        rack_id: id(20),
        overlap_policy: InstrumentRackOverlapPolicy::Layer,
        branches: [
            InstrumentRackBranchSpec {
                branch_id: id(1),
                device_id: id(101),
                gain: 1.0,
                bypassed: false,
                zone: InstrumentRackZone::hard(0, 63, 1, 127),
                state: Some(sub.clone()),
            },
            InstrumentRackBranchSpec {
                branch_id: id(2),
                device_id: id(102),
                gain: 1.0,
                bypassed: false,
                zone: InstrumentRackZone::hard(64, 127, 1, 127),
                state: Some(fm4.clone()),
            },
        ],
    };

    // Independent single-instrument reference projects:
    //   sub_only plays ONLY pitch 60; fm4_only plays ONLY pitch 67!
    let make_single_note_project =
        |inst: NativeDeviceState, pitch: u8, start: BeatTime, vel: u8| {
            let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
            let Created::Track(track) = editor
                .execute(Command::AddMidiTrack { name: "ref".into() })
                .unwrap()
            else {
                panic!("track");
            };
            editor
                .execute(Command::InsertInstrument { track, state: inst })
                .unwrap();
            let Created::Clip(clip) = editor
                .execute(Command::PlaceMidiClip {
                    track,
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
                    start,
                    duration: beat_frac(1, 32),
                    pitch,
                    channel: 0,
                    velocity: vel,
                })
                .unwrap();
            editor
        };

    let sub_ref_editor = make_single_note_project(sub.clone(), 60, BeatTime::ZERO, 110);
    let fm4_ref_editor = make_single_note_project(fm4.clone(), 67, beat_frac(1, 64), 95);
    let (mut rack_editor, source_id, _, _) =
        build_shared_source_project(split_spec.encode_state().unwrap());

    let total_frames = 2_048_usize;
    let mut sub_out = vec![[0.0_f32; 2]; total_frames];
    let mut fm4_out = vec![[0.0_f32; 2]; total_frames];
    let mut rack_out = vec![[0.0_f32; 2]; total_frames];

    let mut sub_pb =
        SessionPlayback::new(prepare_with_rack(sub_ref_editor.project()).unwrap()).unwrap();
    sub_pb.apply(SessionPlaybackCommand::Play).unwrap();
    sub_pb.render(&mut sub_out).unwrap();

    let mut fm4_pb =
        SessionPlayback::new(prepare_with_rack(fm4_ref_editor.project()).unwrap()).unwrap();
    fm4_pb.apply(SessionPlaybackCommand::Play).unwrap();
    fm4_pb.render(&mut fm4_out).unwrap();

    let expected_split: Vec<[f32; 2]> = sub_out
        .iter()
        .zip(&fm4_out)
        .map(|(a, b)| [a[0] + b[0], a[1] + b[1]])
        .collect();

    let rack_rev_split = prepare_with_rack(rack_editor.project()).unwrap();
    let mut rack_pb = SessionPlayback::new(Arc::clone(&rack_rev_split)).unwrap();
    rack_pb.apply(SessionPlaybackCommand::Play).unwrap();
    for chunk in rack_out.chunks_mut(128) {
        rack_pb.render(chunk).unwrap();
    }
    for (idx, (actual, expected)) in rack_out.iter().zip(&expected_split).enumerate() {
        assert!(
            (actual[0] - expected[0]).abs() < 1e-5 && (actual[1] - expected[1]).abs() < 1e-5,
            "zoned split PCM mismatch at frame {idx}: actual={actual:?} expected={expected:?}"
        );
    }

    // Live zone edit while pitch 60 is held (at frame 256, BEFORE pitch 67 arrives at frame 375!):
    // Shrink Branch 1's velocity zone to 100..=127 so pitch 67 (vel 95) is filtered out while
    // already-held pitch 60 on Branch 0 releases cleanly with zero stuck notes!
    let mut edited_spec = split_spec.clone();
    edited_spec.branches[1].zone = InstrumentRackZone::hard(64, 127, 100, 127);
    let rack_device_id = rack_editor
        .project()
        .native_source(source_id)
        .unwrap()
        .instrument()
        .id();
    rack_editor
        .execute(Command::ReplaceDeviceState {
            device: rack_device_id,
            state: edited_spec.encode_state().unwrap(),
        })
        .unwrap();
    let rack_rev_edited = prepare_with_rack(rack_editor.project()).unwrap();

    let mut live_a = SessionPlayback::new(Arc::clone(&rack_rev_split)).unwrap();
    live_a.apply(SessionPlaybackCommand::Play).unwrap();
    let mut live_out = vec![[0.0_f32; 2]; total_frames];
    live_a.render(&mut live_out[..256]).unwrap();

    let mut live_b = SessionPlayback::new(rack_rev_edited).unwrap();
    live_b.try_inherit_state_from(&mut live_a).unwrap();
    live_b.render(&mut live_out[256..]).unwrap();

    // Since pitch 67 (vel 95) arrived AFTER the live velocity-zone edit, live_out MUST match sub_out ONLY!
    for (idx, (actual, expected)) in live_out.iter().zip(&sub_out).enumerate() {
        assert!(
            (actual[0] - expected[0]).abs() < 1e-5 && (actual[1] - expected[1]).abs() < 1e-5,
            "live zone edit PCM mismatch at frame {idx}: actual={actual:?} expected={expected:?}"
        );
    }
    live_b.apply(SessionPlaybackCommand::Stop).unwrap();

    // Undo/Redo + snapshot roundtrip preserve exact zoned rack state!
    rack_editor.undo();
    let reopened = Project::from_snapshot(rack_editor.project().snapshot()).unwrap();
    let reopened_state = reopened
        .native_source(source_id)
        .unwrap()
        .instrument()
        .state();
    assert_eq!(
        InstrumentRackSpec::decode_state(reopened_state).unwrap(),
        split_spec
    );
}

#[test]
fn nested_two_level_shared_source_instrument_rack_matches_pcm_oracle_across_live_retarget_undo_and_stop()
 {
    use hex_lattice::{InstrumentRackOverlapPolicy, InstrumentRackZone};

    let sub = subtractive_state();
    let fm4 = fm4_state();

    // Depth-2 nested Instrument Rack inside ONE shared NativeSource with two contributor tracks:
    //   Outer Branch 0 (gain 0.5) -> Inner Rack (id 200):
    //     Inner Branch 0 (gain 1.0, keys 0..=63):   Subtractive (captures pitch 60!)
    //     Inner Branch 1 (gain 1.0, keys 64..=127): FM4         (captures pitch 67!)
    //   Outer Branch 1 (gain 0.25, full range):     Subtractive (plays BOTH 60 and 67!)
    let make_nested_spec = |inner_b1_zone: InstrumentRackZone| {
        let inner = InstrumentRackSpec {
            rack_id: id(200),
            overlap_policy: InstrumentRackOverlapPolicy::Layer,
            branches: [
                InstrumentRackBranchSpec {
                    branch_id: id(201),
                    device_id: id(21),
                    gain: 1.0,
                    bypassed: false,
                    zone: InstrumentRackZone::hard(0, 63, 1, 127),
                    state: Some(sub.clone()),
                },
                InstrumentRackBranchSpec {
                    branch_id: id(202),
                    device_id: id(22),
                    gain: 1.0,
                    bypassed: false,
                    zone: inner_b1_zone,
                    state: Some(fm4.clone()),
                },
            ],
        };
        InstrumentRackSpec {
            rack_id: id(100),
            overlap_policy: InstrumentRackOverlapPolicy::Layer,
            branches: [
                InstrumentRackBranchSpec {
                    branch_id: id(101),
                    device_id: id(11),
                    gain: 0.5,
                    bypassed: false,
                    zone: InstrumentRackZone::default(),
                    state: Some(inner.encode_state().unwrap()),
                },
                InstrumentRackBranchSpec {
                    branch_id: id(102),
                    device_id: id(12),
                    gain: 0.25,
                    bypassed: false,
                    zone: InstrumentRackZone::default(),
                    state: Some(sub.clone()),
                },
            ],
        }
    };

    let full_spec = make_nested_spec(InstrumentRackZone::hard(64, 127, 1, 127));
    let (mut rack_editor, source_id, _, _) =
        build_shared_source_project(full_spec.encode_state().unwrap());
    assert_eq!(rack_editor.project().native_sources().len(), 1);

    // Independent PCM oracle: 0.5 * (Sub(60) + FM4(67)) + 0.25 * Sub(60, 67).
    let make_single_note_project =
        |inst: NativeDeviceState, pitch: u8, start: BeatTime, vel: u8| {
            let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
            let Created::Track(track) = editor
                .execute(Command::AddMidiTrack { name: "ref".into() })
                .unwrap()
            else {
                panic!("track");
            };
            editor
                .execute(Command::InsertInstrument { track, state: inst })
                .unwrap();
            let Created::Clip(clip) = editor
                .execute(Command::PlaceMidiClip {
                    track,
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
                    start,
                    duration: beat_frac(1, 32),
                    pitch,
                    channel: 0,
                    velocity: vel,
                })
                .unwrap();
            editor
        };

    let total_frames = 2_048_usize;
    let mut sub60_out = vec![[0.0_f32; 2]; total_frames];
    let mut fm67_out = vec![[0.0_f32; 2]; total_frames];
    let mut sub_both_out = vec![[0.0_f32; 2]; total_frames];

    let mut pb1 = SessionPlayback::new(
        prepare_with_rack(make_single_note_project(sub.clone(), 60, BeatTime::ZERO, 110).project())
            .unwrap(),
    )
    .unwrap();
    pb1.apply(SessionPlaybackCommand::Play).unwrap();
    pb1.render(&mut sub60_out).unwrap();

    let mut pb2 = SessionPlayback::new(
        prepare_with_rack(
            make_single_note_project(fm4.clone(), 67, beat_frac(1, 64), 95).project(),
        )
        .unwrap(),
    )
    .unwrap();
    pb2.apply(SessionPlaybackCommand::Play).unwrap();
    pb2.render(&mut fm67_out).unwrap();

    let (sub_both_editor, _, _, _) = build_shared_source_project(sub.clone());
    let mut pb3 =
        SessionPlayback::new(prepare_with_rack(sub_both_editor.project()).unwrap()).unwrap();
    pb3.apply(SessionPlaybackCommand::Play).unwrap();
    pb3.render(&mut sub_both_out).unwrap();

    let expected_full: Vec<[f32; 2]> = (0..total_frames)
        .map(|i| {
            [
                0.5 * (sub60_out[i][0] + fm67_out[i][0]) + 0.25 * sub_both_out[i][0],
                0.5 * (sub60_out[i][1] + fm67_out[i][1]) + 0.25 * sub_both_out[i][1],
            ]
        })
        .collect();

    let rev_full = prepare_with_rack(rack_editor.project()).unwrap();
    let mut actual_full = vec![[0.0_f32; 2]; total_frames];
    let mut rack_pb = SessionPlayback::new(Arc::clone(&rev_full)).unwrap();
    rack_pb.apply(SessionPlaybackCommand::Play).unwrap();
    for chunk in actual_full.chunks_mut(128) {
        rack_pb.render(chunk).unwrap();
    }
    for (idx, (actual, expected)) in actual_full.iter().zip(&expected_full).enumerate() {
        assert!(
            (actual[0] - expected[0]).abs() < 1e-5 && (actual[1] - expected[1]).abs() < 1e-5,
            "nested rack PCM mismatch at frame {idx}: actual={actual:?} expected={expected:?}"
        );
    }

    // Live inner-branch velocity-zone edit while pitch 60 is held (frame 256 < frame 375):
    // excludes FM4 on pitch 67 (vel 95 < 100) while keeping held pitch 60 intact with zero stuck notes!
    let edited_spec = make_nested_spec(InstrumentRackZone::hard(64, 127, 100, 127));
    let rack_dev_id = rack_editor
        .project()
        .native_source(source_id)
        .unwrap()
        .instrument()
        .id();
    rack_editor
        .execute(Command::ReplaceDeviceState {
            device: rack_dev_id,
            state: edited_spec.encode_state().unwrap(),
        })
        .unwrap();
    let rev_edited = prepare_with_rack(rack_editor.project()).unwrap();

    let mut live_a = SessionPlayback::new(Arc::clone(&rev_full)).unwrap();
    live_a.apply(SessionPlaybackCommand::Play).unwrap();
    let mut live_out = vec![[0.0_f32; 2]; total_frames];
    live_a.render(&mut live_out[..256]).unwrap();
    let mut live_b = SessionPlayback::new(rev_edited).unwrap();
    live_b.try_inherit_state_from(&mut live_a).unwrap();
    live_b.render(&mut live_out[256..]).unwrap();
    live_b.apply(SessionPlaybackCommand::Stop).unwrap();

    let expected_no_fm4: Vec<[f32; 2]> = (0..total_frames)
        .map(|i| {
            [
                0.5 * sub60_out[i][0] + 0.25 * sub_both_out[i][0],
                0.5 * sub60_out[i][1] + 0.25 * sub_both_out[i][1],
            ]
        })
        .collect();
    for (idx, (actual, expected)) in live_out.iter().zip(&expected_no_fm4).enumerate() {
        assert!(
            (actual[0] - expected[0]).abs() < 1e-5 && (actual[1] - expected[1]).abs() < 1e-5,
            "live nested retarget mismatch at frame {idx}: actual={actual:?} expected={expected:?}"
        );
    }

    // Undo + snapshot roundtrip preserve exact depth-2 nested Instrument Rack!
    rack_editor.undo();
    let reopened = Project::from_snapshot(rack_editor.project().snapshot()).unwrap();
    let reopened_state = reopened
        .native_source(source_id)
        .unwrap()
        .instrument()
        .state();
    assert_eq!(
        InstrumentRackSpec::decode_state(reopened_state).unwrap(),
        full_spec
    );
}
