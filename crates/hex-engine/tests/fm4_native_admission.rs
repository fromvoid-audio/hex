use hex_devices::SineTable;
use hex_engine::{LaunchQuantization, PreparedRevision, SessionPlayback, SessionPlaybackCommand};
use hex_lattice_contract::DeviceTypeId;
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, f64::consts::TAU, sync::Arc};

const RATE: u32 = 48_000;
const PHASE_SCALE: f64 = 18_446_744_073_709_551_616.0;

fn beat(quarters: u64) -> BeatTime {
    BeatTime::from_quarters(quarters).expect("bounded test beat")
}

fn fixture() -> (Project, hex_project::ClipId, hex_project::TrackId) {
    let mut editor = Editor::new(Project::new(RATE).expect("project"), 16).expect("editor");
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "fm4 route".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("midi track")
    else {
        panic!("midi track result")
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .expect("default instrument slot")
        .id();
    let fm4_type = DeviceTypeId::from_static("hex.instrument.fm4");
    let fm4_state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        fm4_type,
    )
    .expect("built-in FM4 state");
    editor
        .execute(Command::ReplaceDeviceState {
            device: instrument,
            state: fm4_state,
        })
        .expect("replace default sine with FM4");

    let Created::Clip(arrangement_clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: beat(8),
        })
        .expect("Arrangement clip")
    else {
        panic!("Arrangement clip result")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: arrangement_clip,
            start: BeatTime::ZERO,
            duration: beat(2),
            pitch: 69,
            channel: 0,
            velocity: 127,
        })
        .expect("Arrangement note");

    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "fm4 scene".into(),
        })
        .expect("scene")
    else {
        panic!("scene result")
    };
    let Created::SessionClip(session_clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "fm4 session".into(),
            source_length: beat(4),
        })
        .expect("Session clip")
    else {
        panic!("Session clip result")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: session_clip,
            start: BeatTime::ZERO,
            duration: beat(2),
            pitch: 69,
            channel: 0,
            velocity: 127,
        })
        .expect("Session note");
    editor
        .execute(Command::SetMidiClipSource {
            clip: arrangement_clip,
            source_length: beat(4),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: beat(4),
            looping: true,
            length: beat(8),
        })
        .expect("loop Arrangement source for a second FM4 onset");
    (editor.project().clone(), session_clip, track)
}

fn independent_carrier_sample(age: u64) -> f32 {
    let rounded_phase_step = (440.0 / f64::from(RATE) * PHASE_SCALE).round();
    let phase = (rounded_phase_step as u64).wrapping_mul(age);
    let turns = phase as f64 / PHASE_SCALE;
    let attack_frames = 240_u64;
    let decay_frames = 4_800_u64;
    let sustain = 0.7_f64;
    let envelope = if age < attack_frames {
        age as f64 / attack_frames as f64
    } else if age - attack_frames < decay_frames {
        1.0 - (age - attack_frames) as f64 / decay_frames as f64 * (1.0 - sustain)
    } else {
        sustain
    };
    (((turns * TAU).sin() as f32) as f64 * envelope) as f32
}

fn assert_independent_carrier_law(samples: &[[f32; 2]], age_start: u64) {
    for (offset, frame) in samples.iter().enumerate() {
        let age = age_start + offset as u64;
        let expected = independent_carrier_sample(age);
        assert!(
            (frame[0] - expected).abs() < 0.000_02,
            "FM4 carrier mismatch at authored age {age}: actual {}, expected {expected}",
            frame[0]
        );
        assert_eq!(frame[0], frame[1], "default FM4 carrier is centered mono");
    }
}

#[test]
fn built_in_fm4_renders_arrangement_and_session_with_independent_carrier_law() {
    let (project, session_clip, _) = fixture();
    let revision = Arc::new(
        PreparedRevision::prepare(&project, &BTreeMap::new(), Arc::new(SineTable::new()), None)
            .expect("built-in FM4 prepares for both source owners"),
    );

    let mut arrangement = SessionPlayback::new(Arc::clone(&revision)).expect("runtime");
    arrangement
        .apply(SessionPlaybackCommand::Play)
        .expect("play Arrangement");
    let mut whole = [[0.0; 2]; 256];
    arrangement.render(&mut whole).expect("Arrangement render");
    assert_independent_carrier_law(&whole, 0);

    let mut partitioned = SessionPlayback::new(Arc::clone(&revision)).expect("runtime");
    partitioned
        .apply(SessionPlaybackCommand::Play)
        .expect("play partitioned Arrangement");
    let mut split = [[0.0; 2]; 256];
    for part in split.chunks_mut(31) {
        partitioned.render(part).expect("partitioned render");
    }
    assert_eq!(split, whole, "block partitioning preserves FM4 output");

    let mut session = SessionPlayback::new(Arc::clone(&revision)).expect("runtime");
    session
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("launch FM4 Session clip");
    session
        .apply(SessionPlaybackCommand::Play)
        .expect("play Session");
    let mut session_samples = [[0.0; 2]; 256];
    session
        .render(&mut session_samples)
        .expect("Session render");
    assert_independent_carrier_law(&session_samples, 0);
    assert_eq!(session_samples, whole);

    let mut sought_arrangement = SessionPlayback::new(Arc::clone(&revision)).expect("runtime");
    sought_arrangement
        .apply(SessionPlaybackCommand::Seek(96))
        .expect("seek into Arrangement note");
    sought_arrangement
        .apply(SessionPlaybackCommand::Play)
        .expect("resume after seek");
    let mut sought = [[0.0; 2]; 64];
    sought_arrangement.render(&mut sought).expect("seek render");
    assert_independent_carrier_law(&sought, 96);

    let mut looped_arrangement = SessionPlayback::new(revision).expect("runtime");
    looped_arrangement
        .apply(SessionPlaybackCommand::Seek(96_096))
        .expect("seek into second source loop");
    looped_arrangement
        .apply(SessionPlaybackCommand::Play)
        .expect("resume in second source loop");
    let mut looped = [[0.0; 2]; 32];
    looped_arrangement
        .render(&mut looped)
        .expect("render second FM4 occurrence");
    assert_independent_carrier_law(&looped, 96);
}

#[test]
fn ordinary_fm4_oscillator_edit_keeps_active_old_law_notes_in_both_hosts() {
    for session in [false, true] {
        let (project, clip, track) = fixture();
        let table = Arc::new(SineTable::new());
        let first = Arc::new(
            PreparedRevision::prepare(&project, &BTreeMap::new(), Arc::clone(&table), None)
                .unwrap(),
        );
        let mut previous = SessionPlayback::new(Arc::clone(&first)).unwrap();
        previous.apply(SessionPlaybackCommand::Play).unwrap();
        if session {
            previous
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        previous.render(&mut [[0.0; 2]; 128]).unwrap();
        let device = project.track(track).unwrap().instrument().unwrap();
        let state = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            device.state(),
            hex_devices::fm4::device::parameter::oscillator_ratio(1),
            hex_lattice_contract::ParameterValue::Float(1.5),
        )
        .unwrap();
        let device = device.id();
        let mut editor = Editor::new(project, 8).unwrap();
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        let next = Arc::new(
            PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::clone(&table),
                Some(first.session()),
            )
            .unwrap(),
        );
        let mut candidate = SessionPlayback::new(next).unwrap();
        candidate
            .try_inherit_state_from(&mut previous)
            .expect("ordinary FM4 state edits retain the old-law notes");
        let mut output = [[0.0; 2]; 64];
        candidate.render(&mut output).unwrap();
        assert_independent_carrier_law(&output, 128);
    }
}
