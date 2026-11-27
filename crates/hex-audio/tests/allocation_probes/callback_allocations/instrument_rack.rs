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
        overlap_policy: hex_lattice::InstrumentRackOverlapPolicy::Layer,
        branches: [
            InstrumentRackBranchSpec {
                branch_id: id(1),
                device_id: id(101),
                gain: 0.6,
                bypassed: false,
                zone: hex_lattice::InstrumentRackZone::default(),
                state: Some(sub),
            },
            InstrumentRackBranchSpec {
                branch_id: id(2),
                device_id: id(102),
                gain: 0.4,
                bypassed: false,
                zone: hex_lattice::InstrumentRackZone::default(),
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

#[test]
fn zoned_crossfade_instrument_rack_and_live_zone_edit_allocate_nothing_on_callback() {
    use hex_lattice::{InstrumentRackOverlapPolicy, InstrumentRackZone};

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

    let zone_fade_a = InstrumentRackZone {
        key_low: 0,
        key_fade_low: 0,
        key_fade_high: 60,
        key_high: 68,
        vel_low: 1,
        vel_fade_low: 1,
        vel_fade_high: 127,
        vel_high: 127,
    };
    let zone_fade_b = InstrumentRackZone {
        key_low: 56,
        key_fade_low: 64,
        key_fade_high: 127,
        key_high: 127,
        vel_low: 50,
        vel_fade_low: 70,
        vel_fade_high: 127,
        vel_high: 127,
    };
    let spec_a = InstrumentRackSpec {
        rack_id: id(20),
        overlap_policy: InstrumentRackOverlapPolicy::Layer,
        branches: [
            InstrumentRackBranchSpec {
                branch_id: id(1),
                device_id: id(101),
                gain: 0.6,
                bypassed: false,
                zone: zone_fade_a,
                state: Some(sub),
            },
            InstrumentRackBranchSpec {
                branch_id: id(2),
                device_id: id(102),
                gain: 0.4,
                bypassed: false,
                zone: zone_fade_b,
                state: Some(fm4),
            },
        ],
    };
    let mut spec_b = spec_a.clone();
    spec_b.overlap_policy = InstrumentRackOverlapPolicy::ExclusiveFirstMatch;
    spec_b.branches[1].zone = InstrumentRackZone::hard(64, 127, 100, 127);

    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "zoned-rack-probe".into(),
            devices: vec![spec_a.encode_state().unwrap()],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("expected source");
    };
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("expected track");
    };
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
    for (pitch, start, vel) in [(60, BeatTime::ZERO, 110), (67, beat_frac(1, 64), 95)] {
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
    }

    let builtin = hex_devices::native_instrument_registry().unwrap();
    let mut defs: Vec<&dyn NativeInstrumentDefinition> = builtin.definitions().to_vec();
    let rack_def = InstrumentRackDefinition::new(builtin).unwrap();
    defs.push(&rack_def);
    let combined = NativeInstrumentRegistry::new(&defs).unwrap();
    let table = Arc::new(SineTable::new());
    let rev_a = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            None,
            &combined,
        )
        .unwrap(),
    );
    let inst_id = editor
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device: inst_id,
            state: spec_b.encode_state().unwrap(),
        })
        .unwrap();
    let rev_b = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            None,
            &combined,
        )
        .unwrap(),
    );

    let mut active = SessionPlayback::new(Arc::clone(&rev_a)).unwrap();
    let mut edited = SessionPlayback::new(Arc::clone(&rev_b)).unwrap();
    let mut buf = vec![[0.0_f32; 2]; 256];

    let info = measure(|| {
        active.apply(SessionPlaybackCommand::Play).unwrap();
        active.render(&mut buf).unwrap();
        edited.try_inherit_state_from(&mut active).unwrap();
        edited.render(&mut buf).unwrap();
        edited.apply(SessionPlaybackCommand::Stop).unwrap();
    });

    assert_eq!(
        info.count_total, 0,
        "zoned rack callback allocated: {info:?}"
    );
    assert_eq!(info.bytes_total, 0, "zoned rack callback bytes: {info:?}");
    assert_eq!(info.count_current, 0, "zoned rack net count: {info:?}");
    assert_eq!(info.bytes_current, 0, "zoned rack net bytes: {info:?}");

    drop(edited);
    drop(active);
    drop(rev_b);
    drop(rev_a);
    drop(table);
}

#[test]
fn nested_two_level_shared_source_instrument_rack_allocates_nothing_on_callback() {
    use hex_lattice::{InstrumentRackOverlapPolicy, InstrumentRackZone};

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

    let make_nested = |inner_b1_zone: InstrumentRackZone| {
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

    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "nested-rack-probe".into(),
            devices: vec![
                make_nested(InstrumentRackZone::hard(64, 127, 1, 127))
                    .encode_state()
                    .unwrap(),
            ],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("expected source");
    };
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("expected track");
    };
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
    for (pitch, start, vel) in [(60, BeatTime::ZERO, 110), (67, beat_frac(1, 64), 95)] {
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
    }

    let builtin = hex_devices::native_instrument_registry().unwrap();
    let mut defs: Vec<&dyn NativeInstrumentDefinition> = builtin.definitions().to_vec();
    let rack_def = InstrumentRackDefinition::new(builtin).unwrap();
    defs.push(&rack_def);
    let combined = NativeInstrumentRegistry::new(&defs).unwrap();
    let table = Arc::new(SineTable::new());
    let rev_a = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            None,
            &combined,
        )
        .unwrap(),
    );
    let inst_id = editor
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device: inst_id,
            state: make_nested(InstrumentRackZone::hard(64, 127, 100, 127))
                .encode_state()
                .unwrap(),
        })
        .unwrap();
    let rev_b = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            None,
            &combined,
        )
        .unwrap(),
    );

    let mut active = SessionPlayback::new(Arc::clone(&rev_a)).unwrap();
    let mut edited = SessionPlayback::new(Arc::clone(&rev_b)).unwrap();
    let mut buf = vec![[0.0_f32; 2]; 256];

    let info = measure(|| {
        active.apply(SessionPlaybackCommand::Play).unwrap();
        active.render(&mut buf).unwrap();
        edited.try_inherit_state_from(&mut active).unwrap();
        edited.render(&mut buf).unwrap();
        edited.apply(SessionPlaybackCommand::Stop).unwrap();
    });

    assert_eq!(
        info.count_total, 0,
        "nested rack callback allocated: {info:?}"
    );
    assert_eq!(info.bytes_total, 0, "nested rack callback bytes: {info:?}");
    assert_eq!(info.count_current, 0, "nested rack net count: {info:?}");
    assert_eq!(info.bytes_current, 0, "nested rack net bytes: {info:?}");

    drop(edited);
    drop(active);
    drop(rev_b);
    drop(rev_a);
    drop(table);
}
