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
        branches: [
            InstrumentRackBranchSpec {
                branch_id: id(1),
                device_id: id(101),
                gain: 0.6,
                bypassed: false,
                state: Some(sub.clone()),
            },
            InstrumentRackBranchSpec {
                branch_id: id(2),
                device_id: id(102),
                gain: 0.4,
                bypassed: false,
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
