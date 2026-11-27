use allocation_counter::measure;
use hex_engine::{
    LaunchQuantization, NativeDeviceClass, PreparedRevision, SessionPlayback,
    SessionPlaybackCommand, SineTable, default_native_device_state_for_class,
    patch_native_device_parameter_for_class,
};
use hex_lattice::conformance::midi_transpose::{
    PreparedTransposeMidiProcessor, TransposeOutOfRangePolicy,
};
use hex_lattice_contract::ParameterValue;
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, num::NonZeroU64, sync::Arc};

const RATE: u32 = 48_000;

fn beat(quarter_notes: u64) -> BeatTime {
    BeatTime::from_raw(quarter_notes << 32)
}

fn beat_frac(num: u64, den: u64) -> BeatTime {
    BeatTime::from_raw((num << 32) / den)
}

fn subtractive_state() -> hex_lattice_contract::NativeDeviceState {
    let base = default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        hex_devices::subtractive::SUBTRACTIVE_TYPE_ID,
    )
    .unwrap();
    let attack = patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &base,
        hex_devices::subtractive::parameter::ATTACK,
        ParameterValue::Float(0.001),
    )
    .unwrap();
    patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &attack,
        hex_devices::subtractive::parameter::RELEASE,
        ParameterValue::Float(0.005),
    )
    .unwrap()
}

fn build_project() -> (Editor, hex_project::TrackId, hex_project::ClipId) {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "midi-proc-probe".into(),
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
        .execute(Command::InsertInstrument {
            track,
            state: subtractive_state(),
        })
        .unwrap();
    let Created::Clip(arr_clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: beat(2),
        })
        .unwrap()
    else {
        panic!("expected arrangement clip");
    };
    // Overlapping same-pitch C4 notes.
    editor
        .execute(Command::AddMidiNote {
            clip: arr_clip,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 32),
            pitch: 60,
            channel: 0,
            velocity: 110,
        })
        .unwrap();
    editor
        .execute(Command::AddMidiNote {
            clip: arr_clip,
            start: beat_frac(1, 64),
            duration: beat_frac(1, 32),
            pitch: 60,
            channel: 0,
            velocity: 90,
        })
        .unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 1_500,
            enabled: true,
        })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene 1".into(),
        })
        .unwrap()
    else {
        panic!("expected scene");
    };
    let Created::SessionClip(session_clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "session-midi".into(),
            source_length: beat_frac(1, 16),
        })
        .unwrap()
    else {
        panic!("expected session clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: session_clip,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 32),
            pitch: 64,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    (editor, track, session_clip)
}

#[test]
fn pre_instrument_midi_processor_route_and_handoff_allocate_nothing_on_callback() {
    let (editor, track, session_clip) = build_project();

    // All preparation, admission, and buffer allocation stay strictly on control.
    let table = Arc::new(SineTable::new());
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );
    let transpose =
        PreparedTransposeMidiProcessor::new(7, TransposeOutOfRangePolicy::Refuse).unwrap();
    let proc_id = NonZeroU64::new(42).unwrap();

    let mut active = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    active
        .install_track_midi_processor_on_control(track, proc_id, &transpose)
        .unwrap();
    let mut candidate = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    candidate
        .install_track_midi_processor_on_control(track, proc_id, &transpose)
        .unwrap();

    let mut block_a = vec![[0.0_f32; 2]; 256];
    let mut block_b = vec![[0.0_f32; 2]; 256];
    let mut block_c = vec![[0.0_f32; 2]; 1_024];

    // Measure pure callback execution: Play, loop wrap, compatible mid-note handoff,
    // Session launch, StopTrack, BackToArrangement, Stop, Seek(0), and replay.
    let info = measure(|| {
        active.apply(SessionPlaybackCommand::Play).unwrap();
        active.render(&mut block_a).unwrap();

        candidate.try_inherit_state_from(&mut active).unwrap();
        for _ in 0..8 {
            candidate.render(&mut block_b).unwrap();
        }
        candidate
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: session_clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        candidate.render(&mut block_c).unwrap();
        candidate
            .apply(SessionPlaybackCommand::StopTrack {
                track,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        candidate.render(&mut block_c).unwrap();
        candidate
            .apply(SessionPlaybackCommand::BackToArrangement {
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        candidate.apply(SessionPlaybackCommand::Stop).unwrap();
        candidate.apply(SessionPlaybackCommand::Seek(0)).unwrap();
        candidate.apply(SessionPlaybackCommand::Play).unwrap();
        candidate.render(&mut block_b).unwrap();
    });

    assert_eq!(
        info.count_total, 0,
        "callback must allocate nothing: {info:?}"
    );
    assert_eq!(
        info.bytes_total, 0,
        "callback must allocate zero bytes: {info:?}"
    );
    assert_eq!(
        info.count_current, 0,
        "callback signed-net allocation count must be zero: {info:?}"
    );
    assert_eq!(
        info.bytes_current, 0,
        "callback signed-net allocation bytes must be zero: {info:?}"
    );

    // Confirm real audio was rendered and final replay reproduces block_a bit-for-bit.
    assert!(block_a.iter().flatten().any(|s| s.abs() > 1e-3));
    assert_eq!(block_b, block_a);

    // Control-side retirement outside measured scope.
    drop(candidate);
    drop(active);
    drop(revision);
    drop(table);
}

#[test]
fn canonical_pitch_midi_processor_transposition_and_bypass_handoff_allocate_nothing_on_callback() {
    let (mut editor, track, _session_clip) = build_project();
    let pitch_7 = hex_devices::midi_pitch::PitchSettings {
        semitones: 7,
        ..Default::default()
    }
    .encode()
    .unwrap();
    let Created::Device(pitch_id) = editor
        .execute(Command::InsertMidiProcessor {
            track,
            state: pitch_7,
        })
        .unwrap()
    else {
        panic!("expected midi processor id");
    };

    let table = Arc::new(SineTable::new());
    let rev_7 = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    let pitch_12 = hex_devices::midi_pitch::PitchSettings {
        semitones: 12,
        ..Default::default()
    }
    .encode()
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: pitch_id,
            state: pitch_12,
        })
        .unwrap();
    let rev_12 = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    editor
        .execute(Command::SetDeviceBypass {
            device: pitch_id,
            bypassed: true,
        })
        .unwrap();
    let rev_bypassed = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    let mut active = SessionPlayback::new(Arc::clone(&rev_7)).unwrap();
    let mut shifted = SessionPlayback::new(Arc::clone(&rev_12)).unwrap();
    let mut bypassed = SessionPlayback::new(Arc::clone(&rev_bypassed)).unwrap();
    let mut buf = vec![[0.0_f32; 2]; 256];

    let info = measure(|| {
        active.apply(SessionPlaybackCommand::Play).unwrap();
        active.render(&mut buf).unwrap();
        // Mid-note transposition change (+7 -> +12)
        shifted.try_inherit_state_from(&mut active).unwrap();
        shifted.render(&mut buf).unwrap();
        // Mid-note bypass toggle
        bypassed.try_inherit_state_from(&mut shifted).unwrap();
        bypassed.render(&mut buf).unwrap();
        bypassed.apply(SessionPlaybackCommand::Stop).unwrap();
    });

    assert_eq!(info.count_total, 0, "callback allocated: {info:?}");
    assert_eq!(info.bytes_total, 0, "callback allocated bytes: {info:?}");
    assert_eq!(info.count_current, 0, "callback net count: {info:?}");
    assert_eq!(info.bytes_current, 0, "callback net bytes: {info:?}");
    assert!(buf.iter().flatten().any(|s| s.abs() > 1e-3));

    drop(bypassed);
    drop(shifted);
    drop(active);
    drop(rev_bypassed);
    drop(rev_12);
    drop(rev_7);
    drop(table);
}

#[test]
fn canonical_chord_midi_processor_fanout_voicing_edit_and_bypass_allocate_nothing_on_callback() {
    let (mut editor, track, _session_clip) = build_project();
    let major = hex_devices::midi_chord::ChordSettings {
        shifts: [4, 7, 12, 0, 0, 0],
        ..Default::default()
    }
    .encode()
    .unwrap();
    let Created::Device(chord_id) = editor
        .execute(Command::InsertMidiProcessor {
            track,
            state: major,
        })
        .unwrap()
    else {
        panic!("expected chord processor id");
    };

    let table = Arc::new(SineTable::new());
    let rev_major = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    let minor = hex_devices::midi_chord::ChordSettings {
        shifts: [3, 7, 10, 0, 0, 0],
        ..Default::default()
    }
    .encode()
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: chord_id,
            state: minor,
        })
        .unwrap();
    let rev_minor = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    editor
        .execute(Command::SetDeviceBypass {
            device: chord_id,
            bypassed: true,
        })
        .unwrap();
    let rev_bypassed = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    let mut active = SessionPlayback::new(Arc::clone(&rev_major)).unwrap();
    let mut voiced = SessionPlayback::new(Arc::clone(&rev_minor)).unwrap();
    let mut bypassed = SessionPlayback::new(Arc::clone(&rev_bypassed)).unwrap();
    let mut buf = vec![[0.0_f32; 2]; 256];

    let info = measure(|| {
        active.apply(SessionPlaybackCommand::Play).unwrap();
        active.render(&mut buf).unwrap();
        // Mid-note chord voicing change (major -> minor)
        voiced.try_inherit_state_from(&mut active).unwrap();
        voiced.render(&mut buf).unwrap();
        // Mid-note bypass toggle
        bypassed.try_inherit_state_from(&mut voiced).unwrap();
        bypassed.render(&mut buf).unwrap();
        bypassed.apply(SessionPlaybackCommand::Stop).unwrap();
    });

    assert_eq!(info.count_total, 0, "callback allocated: {info:?}");
    assert_eq!(info.bytes_total, 0, "callback allocated bytes: {info:?}");
    assert_eq!(info.count_current, 0, "callback net count: {info:?}");
    assert_eq!(info.bytes_current, 0, "callback net bytes: {info:?}");
    assert!(buf.iter().flatten().any(|s| s.abs() > 1e-3));

    drop(bypassed);
    drop(voiced);
    drop(active);
    drop(rev_bypassed);
    drop(rev_minor);
    drop(rev_major);
    drop(table);
}

#[test]
fn canonical_velocity_midi_processor_mapping_live_edit_and_bypass_allocate_nothing_on_callback() {
    let (mut editor, track, _session_clip) = build_project();
    let fixed = hex_devices::midi_velocity::VelocitySettings {
        mode: hex_devices::midi_velocity::VelocityMode::Fixed,
        out_high: 100,
        ..Default::default()
    }
    .encode()
    .unwrap();
    let Created::Device(vel_id) = editor
        .execute(Command::InsertMidiProcessor {
            track,
            state: fixed,
        })
        .unwrap()
    else {
        panic!("expected velocity processor id");
    };

    let table = Arc::new(SineTable::new());
    let rev_fixed = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    let gated = hex_devices::midi_velocity::VelocitySettings {
        mode: hex_devices::midi_velocity::VelocityMode::Gate,
        lowest: 60,
        range: 67,
        ..Default::default()
    }
    .encode()
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: vel_id,
            state: gated,
        })
        .unwrap();
    let rev_gated = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    editor
        .execute(Command::SetDeviceBypass {
            device: vel_id,
            bypassed: true,
        })
        .unwrap();
    let rev_bypassed = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    let mut active = SessionPlayback::new(Arc::clone(&rev_fixed)).unwrap();
    let mut next = SessionPlayback::new(Arc::clone(&rev_gated)).unwrap();
    let mut bypassed = SessionPlayback::new(Arc::clone(&rev_bypassed)).unwrap();
    let mut buf = vec![[0.0_f32; 2]; 256];

    let info = measure(|| {
        active.apply(SessionPlaybackCommand::Play).unwrap();
        active.render(&mut buf).unwrap();
        next.try_inherit_state_from(&mut active).unwrap();
        next.render(&mut buf).unwrap();
        bypassed.try_inherit_state_from(&mut next).unwrap();
        bypassed.render(&mut buf).unwrap();
        bypassed.apply(SessionPlaybackCommand::Stop).unwrap();
    });

    assert_eq!(info.count_total, 0, "callback allocated: {info:?}");
    assert_eq!(info.bytes_total, 0, "callback allocated bytes: {info:?}");
    assert_eq!(info.count_current, 0, "callback net count: {info:?}");
    assert_eq!(info.bytes_current, 0, "callback net bytes: {info:?}");
    assert!(buf.iter().flatten().any(|s| s.abs() > 1e-3));

    drop(bypassed);
    drop(next);
    drop(active);
    drop(rev_bypassed);
    drop(rev_gated);
    drop(rev_fixed);
    drop(table);
}

#[test]
fn canonical_scale_midi_processor_mapping_live_edit_and_bypass_allocate_nothing_on_callback() {
    let (mut editor, track, _session_clip) = build_project();
    let major = hex_devices::midi_scale::ScaleSettings {
        base: 0,
        degree_map: [0, 0, 2, 2, 4, 5, 5, 7, 7, 9, 9, 11],
        ..Default::default()
    }
    .encode()
    .unwrap();
    let Created::Device(scale_id) = editor
        .execute(Command::InsertMidiProcessor {
            track,
            state: major,
        })
        .unwrap()
    else {
        panic!("expected scale processor id");
    };

    let table = Arc::new(SineTable::new());
    let rev_a = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    let minor = hex_devices::midi_scale::ScaleSettings {
        base: 0,
        degree_map: [0, 0, 2, 3, 3, 5, 5, 7, 8, 8, 10, 10],
        ..Default::default()
    }
    .encode()
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: scale_id,
            state: minor,
        })
        .unwrap();
    let rev_b = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    editor
        .execute(Command::SetDeviceBypass {
            device: scale_id,
            bypassed: true,
        })
        .unwrap();
    let rev_bypassed = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    let mut active = SessionPlayback::new(Arc::clone(&rev_a)).unwrap();
    let mut next = SessionPlayback::new(Arc::clone(&rev_b)).unwrap();
    let mut bypassed = SessionPlayback::new(Arc::clone(&rev_bypassed)).unwrap();
    let mut buf = vec![[0.0_f32; 2]; 256];

    let info = measure(|| {
        active.apply(SessionPlaybackCommand::Play).unwrap();
        active.render(&mut buf).unwrap();
        next.try_inherit_state_from(&mut active).unwrap();
        next.render(&mut buf).unwrap();
        bypassed.try_inherit_state_from(&mut next).unwrap();
        bypassed.render(&mut buf).unwrap();
        bypassed.apply(SessionPlaybackCommand::Stop).unwrap();
    });

    assert_eq!(info.count_total, 0, "callback allocated: {info:?}");
    assert_eq!(info.bytes_total, 0, "callback allocated bytes: {info:?}");
    assert_eq!(info.count_current, 0, "callback net count: {info:?}");
    assert_eq!(info.bytes_current, 0, "callback net bytes: {info:?}");
    assert!(buf.iter().flatten().any(|s| s.abs() > 1e-3));

    drop(bypassed);
    drop(next);
    drop(active);
    drop(rev_bypassed);
    drop(rev_b);
    drop(rev_a);
    drop(table);
}

#[test]
fn canonical_random_midi_processor_seeded_live_edit_and_bypass_allocate_nothing_on_callback() {
    let (mut editor, track, _session_clip) = build_project();
    let seed_a = hex_devices::midi_random::RandomSettings {
        chance_bps: hex_devices::midi_random::MAX_CHANCE_BPS,
        choices: 6,
        scale: 2,
        sign: hex_devices::midi_random::RandomSign::Bi,
        seed: 77,
        ..Default::default()
    }
    .encode()
    .unwrap();
    let Created::Device(random_id) = editor
        .execute(Command::InsertMidiProcessor {
            track,
            state: seed_a,
        })
        .unwrap()
    else {
        panic!("expected random processor id");
    };

    let table = Arc::new(SineTable::new());
    let rev_a = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    let seed_b = hex_devices::midi_random::RandomSettings {
        chance_bps: hex_devices::midi_random::MAX_CHANCE_BPS,
        choices: 6,
        scale: 2,
        sign: hex_devices::midi_random::RandomSign::Bi,
        seed: 99,
        ..Default::default()
    }
    .encode()
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: random_id,
            state: seed_b,
        })
        .unwrap();
    let rev_b = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    editor
        .execute(Command::SetDeviceBypass {
            device: random_id,
            bypassed: true,
        })
        .unwrap();
    let rev_bypassed = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    let mut active = SessionPlayback::new(Arc::clone(&rev_a)).unwrap();
    let mut reseeded = SessionPlayback::new(Arc::clone(&rev_b)).unwrap();
    let mut bypassed = SessionPlayback::new(Arc::clone(&rev_bypassed)).unwrap();
    let mut buf = vec![[0.0_f32; 2]; 256];

    let info = measure(|| {
        active.apply(SessionPlaybackCommand::Play).unwrap();
        active.render(&mut buf).unwrap();
        reseeded.try_inherit_state_from(&mut active).unwrap();
        reseeded.render(&mut buf).unwrap();
        bypassed.try_inherit_state_from(&mut reseeded).unwrap();
        bypassed.render(&mut buf).unwrap();
        bypassed.apply(SessionPlaybackCommand::Stop).unwrap();
    });

    assert_eq!(info.count_total, 0, "callback allocated: {info:?}");
    assert_eq!(info.bytes_total, 0, "callback allocated bytes: {info:?}");
    assert_eq!(info.count_current, 0, "callback net count: {info:?}");
    assert_eq!(info.bytes_current, 0, "callback net bytes: {info:?}");
    assert!(buf.iter().flatten().any(|s| s.abs() > 1e-3));

    drop(bypassed);
    drop(reseeded);
    drop(active);
    drop(rev_bypassed);
    drop(rev_b);
    drop(rev_a);
    drop(table);
}

#[test]
fn bounded_control_change_remapping_and_fm4_consumption_allocate_nothing_on_callback() {
    use hex_lattice::conformance::midi_cc_remap::PreparedCcRemapMidiProcessor;
    use hex_project::{MidiLaneKind, MidiLanePoint};

    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "cc-probe".into(),
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
    let base = default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        hex_devices::fm4::device::FM4_TYPE_ID,
    )
    .unwrap();
    let with_mod = patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &base,
        hex_devices::fm4::device::parameter::modulation(2, 1),
        ParameterValue::Float(0.6),
    )
    .unwrap();
    editor
        .execute(Command::InsertInstrument {
            track,
            state: with_mod,
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
        panic!("expected clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 16),
            pitch: 60,
            channel: 0,
            velocity: 110,
        })
        .unwrap();
    editor
        .execute(Command::SetMidiExpressionLane {
            clip,
            kind: MidiLaneKind::ControlChange(20),
            points: vec![
                MidiLanePoint {
                    beat: BeatTime::ZERO,
                    channel: 0,
                    value: 127,
                },
                MidiLanePoint {
                    beat: beat_frac(1, 64),
                    channel: 0,
                    value: 32,
                },
            ],
        })
        .unwrap();

    let table = Arc::new(SineTable::new());
    let rev = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );
    let remap = PreparedCcRemapMidiProcessor::new(20, 74).unwrap();
    let proc_id = NonZeroU64::new(77).unwrap();

    let mut active = SessionPlayback::new(Arc::clone(&rev)).unwrap();
    active
        .install_track_midi_processor_on_control(track, proc_id, &remap)
        .unwrap();
    let mut next = SessionPlayback::new(Arc::clone(&rev)).unwrap();
    next.install_track_midi_processor_on_control(track, proc_id, &remap)
        .unwrap();
    let mut buf = vec![[0.0_f32; 2]; 256];

    let info = measure(|| {
        active.apply(SessionPlaybackCommand::Play).unwrap();
        active.render(&mut buf).unwrap();
        next.try_inherit_state_from(&mut active).unwrap();
        next.render(&mut buf).unwrap();
        next.apply(SessionPlaybackCommand::Stop).unwrap();
    });

    assert_eq!(info.count_total, 0, "callback allocated: {info:?}");
    assert_eq!(info.bytes_total, 0, "callback allocated bytes: {info:?}");
    assert_eq!(info.count_current, 0, "callback net count: {info:?}");
    assert_eq!(info.bytes_current, 0, "callback net bytes: {info:?}");
    assert!(buf.iter().flatten().any(|s| s.abs() > 1e-3));

    drop(next);
    drop(active);
    drop(rev);
    drop(table);
}

#[test]
fn canonical_cc_control_midi_processor_live_edit_slew_and_bypass_allocate_nothing_on_callback() {
    use hex_devices::midi_cc_control::{CcControlSettings, CcLaneSettings};

    let (mut editor, track, _session_clip) = build_project();
    let state_a = CcControlSettings {
        slew: 24,
        lanes: [
            CcLaneSettings {
                enabled: true,
                cc: 74,
                value: 127,
                min: 0,
                max: 127,
            },
            CcControlSettings::default().lanes[1],
        ],
        ..Default::default()
    }
    .encode()
    .unwrap();
    let Created::Device(cc_id) = editor
        .execute(Command::InsertMidiProcessor {
            track,
            state: state_a,
        })
        .unwrap()
    else {
        panic!("expected cc control processor id");
    };

    let table = Arc::new(SineTable::new());
    let rev_a = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    let state_b = CcControlSettings {
        slew: 24,
        lanes: [
            CcLaneSettings {
                enabled: true,
                cc: 74,
                value: 0,
                min: 0,
                max: 127,
            },
            CcControlSettings::default().lanes[1],
        ],
        ..Default::default()
    }
    .encode()
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: cc_id,
            state: state_b,
        })
        .unwrap();
    let rev_b = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    editor
        .execute(Command::SetDeviceBypass {
            device: cc_id,
            bypassed: true,
        })
        .unwrap();
    let rev_bypassed = Arc::new(
        PreparedRevision::prepare(editor.project(), &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap(),
    );

    let mut active = SessionPlayback::new(Arc::clone(&rev_a)).unwrap();
    let mut slewed = SessionPlayback::new(Arc::clone(&rev_b)).unwrap();
    let mut bypassed = SessionPlayback::new(Arc::clone(&rev_bypassed)).unwrap();
    let mut buf = vec![[0.0_f32; 2]; 256];

    let info = measure(|| {
        active.apply(SessionPlaybackCommand::Play).unwrap();
        active.render(&mut buf).unwrap();
        slewed.try_inherit_state_from(&mut active).unwrap();
        slewed.render(&mut buf).unwrap();
        slewed.render(&mut buf).unwrap();
        bypassed.try_inherit_state_from(&mut slewed).unwrap();
        bypassed.render(&mut buf).unwrap();
        bypassed.apply(SessionPlaybackCommand::Stop).unwrap();
    });

    assert_eq!(info.count_total, 0, "callback allocated: {info:?}");
    assert_eq!(info.bytes_total, 0, "callback allocated bytes: {info:?}");
    assert_eq!(info.count_current, 0, "callback net count: {info:?}");
    assert_eq!(info.bytes_current, 0, "callback net bytes: {info:?}");
    assert!(buf.iter().flatten().any(|s| s.abs() > 1e-3));

    drop(bypassed);
    drop(slewed);
    drop(active);
    drop(rev_bypassed);
    drop(rev_b);
    drop(rev_a);
    drop(table);
}
