use allocation_counter::measure;
use hex_engine::{
    NativeDeviceClass, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable,
    default_native_device_state_for_class, patch_native_device_parameter_for_class,
};
use hex_lattice::{
    InstrumentRackBranchSpec, InstrumentRackDefinition, InstrumentRackSpec,
    NativeInstrumentDefinition, NativeInstrumentRegistry,
};
use hex_lattice_contract::ParameterValue;
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, num::NonZeroU64, sync::Arc};

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

#[test]
fn two_branch_shared_source_instrument_rack_allocates_nothing_on_callback() {
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
        ParameterValue::Float(0.005),
    )
    .unwrap();
    let fm4 = default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        hex_devices::fm4::device::FM4_TYPE_ID,
    )
    .unwrap();

    let rack_state = InstrumentRackSpec {
        rack_id: id(10),
        branches: [
            InstrumentRackBranchSpec {
                branch_id: id(1),
                device_id: id(101),
                gain: 0.6,
                bypassed: false,
                state: Some(sub),
            },
            InstrumentRackBranchSpec {
                branch_id: id(2),
                device_id: id(102),
                gain: 0.4,
                bypassed: false,
                state: Some(fm4),
            },
        ],
    }
    .encode_state()
    .unwrap();

    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "rack-probe".into(),
            devices: vec![rack_state],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("expected source");
    };
    let Created::Track(track_a) = editor
        .execute(Command::AddMidiTrack {
            name: "track-a".into(),
        })
        .unwrap()
    else {
        panic!("expected track a");
    };
    let Created::Track(track_b) = editor
        .execute(Command::AddMidiTrack {
            name: "track-b".into(),
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

    // Control-side preparation & preallocation strictly outside callback measurement.
    let builtin = hex_devices::native_instrument_registry().unwrap();
    let mut defs: Vec<&dyn NativeInstrumentDefinition> = builtin.definitions().to_vec();
    let rack_def = InstrumentRackDefinition::new(builtin).unwrap();
    defs.push(&rack_def);
    let combined = NativeInstrumentRegistry::new(&defs).unwrap();
    let table = Arc::new(SineTable::new());
    let revision = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            None,
            &combined,
        )
        .unwrap(),
    );

    let mut active = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut candidate = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut block_a = vec![[0.0_f32; 2]; 256];
    let mut block_b = vec![[0.0_f32; 2]; 256];

    let info = measure(|| {
        active.apply(SessionPlaybackCommand::Play).unwrap();
        active.render(&mut block_a).unwrap();

        candidate.try_inherit_state_from(&mut active).unwrap();
        for _ in 0..8 {
            candidate.render(&mut block_b).unwrap();
        }
        candidate.apply(SessionPlaybackCommand::Stop).unwrap();
        candidate.apply(SessionPlaybackCommand::Seek(0)).unwrap();
        candidate.apply(SessionPlaybackCommand::Play).unwrap();
        candidate.render(&mut block_b).unwrap();
    });

    assert_eq!(
        info.count_total, 0,
        "instrument rack callback must allocate nothing: {info:?}"
    );
    assert_eq!(
        info.bytes_total, 0,
        "instrument rack callback must allocate zero bytes: {info:?}"
    );
    assert_eq!(
        info.count_current, 0,
        "instrument rack signed-net count must be zero: {info:?}"
    );
    assert_eq!(
        info.bytes_current, 0,
        "instrument rack signed-net bytes must be zero: {info:?}"
    );
    assert!(block_a.iter().flatten().any(|s| s.abs() > 1e-3));
    assert_eq!(block_b, block_a);

    // Control-side retirement strictly outside measured callback scope.
    drop(candidate);
    drop(active);
    drop(revision);
    drop(table);
}
