use hex_devices::SineTable;
use hex_engine::{PreparedRevision, Renderer, SessionPlayback, SessionPlaybackCommand};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, f64::consts::TAU, sync::Arc};

const RATE: u32 = 48_000;
const RELEASE_FRAMES: u64 = 240;
const LOOP_FRAMES: u64 = 12_000;
const OPERATOR: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.operator");

#[test]
fn fixed_operator_is_rendered_at_effective_frequency_by_the_shared_project_path() {
    let mut fixture = fixture();
    let mut state = operator_state(1.0);
    for (id, value) in [
        (63, ParameterValue::Float(125.0)),
        (64, ParameterValue::Float(2.0)),
        (62, ParameterValue::Bool(true)),
    ] {
        state = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            &state,
            ParameterId::from_static(id),
            value,
        )
        .expect("fixed patch");
    }
    fixture
        .editor
        .execute(Command::ReplaceDeviceState {
            device: fixture.instrument,
            state,
        })
        .expect("fixed state");
    let revision = prepared(&fixture, None);
    let mut playback =
        hex_engine::playback::Playback::try_from_prepared_with_session(revision).expect("playback");
    playback.apply(hex_engine::playback::PlaybackCommand::Play);
    let mut frames = [[0.0; 2]; 1600];
    playback.render(&mut frames).expect("fixed render");
    for (index, frame) in frames.iter().enumerate() {
        let expected = (TAU * 250.0 * index as f64 / 48_000.0).sin() as f32;
        assert!((frame[0] - expected).abs() < 0.000_02, "frame{index}");
        assert_eq!(frame[0], frame[1]);
    }
}

fn beat(numerator: u64, denominator: u64) -> BeatTime {
    BeatTime::from_ratio(numerator, denominator).expect("bounded test beat")
}

struct Fixture {
    editor: Editor,
    track: hex_project::TrackId,
    arrangement_clip: hex_project::ClipId,
    session_clip: hex_project::ClipId,
    instrument: hex_project::DeviceId,
}

fn operator_state(carrier: f32) -> hex_lattice_contract::NativeDeviceState {
    let mut state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        OPERATOR,
    )
    .expect("registered Operator defaults");
    for (parameter, value) in [
        (29, carrier), // op1 is the only default carrier
        (33, 1.0),     // initial
        (34, 1.0),     // peak
        (35, 1.0),     // sustain
        (36, 0.0),     // attack
        (37, 0.0),     // decay
        (38, 0.005),   // release
    ] {
        state = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            &state,
            ParameterId::from_static(parameter),
            ParameterValue::Float(value),
        )
        .expect("valid Operator patch");
    }
    state
}

fn fixture() -> Fixture {
    let mut editor = Editor::new(Project::new(RATE).expect("project"), 16).expect("editor");
    let track = match editor
        .execute(Command::AddMidiTrack {
            name: "operator".into(),
        })
        .expect("midi track")
    {
        Created::Track(track) => track,
        _ => panic!("expected midi track"),
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .expect("default instrument slot")
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device: instrument,
            state: operator_state(1.0),
        })
        .expect("register Operator state in the occupied slot");
    let arrangement_clip = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: beat(8, 1),
        })
        .expect("Arrangement clip")
    {
        Created::Clip(clip) => clip,
        _ => panic!("expected Arrangement clip"),
    };
    for (start, duration) in [(BeatTime::ZERO, beat(1, 4)), (beat(1, 1), beat(1, 4))] {
        editor
            .execute(Command::AddMidiNote {
                clip: arrangement_clip,
                start,
                duration,
                pitch: 69,
                channel: 0,
                velocity: 127,
            })
            .expect("Arrangement note");
    }
    let scene = match editor
        .execute(Command::AddScene {
            name: "operator".into(),
        })
        .expect("Session scene")
    {
        Created::Scene(scene) => scene,
        _ => panic!("expected scene"),
    };
    let session_clip = match editor
        .execute(Command::CopyArrangementMidiToSession {
            source: arrangement_clip,
            scene,
        })
        .expect("copy Arrangement notes into Session")
    {
        Created::SessionClip(clip) => clip,
        _ => panic!("expected Session clip"),
    };
    Fixture {
        editor,
        track,
        arrangement_clip,
        session_clip,
        instrument,
    }
}

fn prepared(
    fixture: &Fixture,
    previous: Option<&hex_engine::PreparedSessionInventory>,
) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(
            fixture.editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            previous,
        )
        .expect("built-in Operator compiles through shared contexts"),
    )
}

fn expected_note(age: u64, gate: u64, carrier: f32) -> f32 {
    let release_age = age.saturating_sub(gate);
    let amplitude = if age < gate {
        1.0
    } else if release_age >= RELEASE_FRAMES {
        0.0
    } else {
        0.000_01_f64.powf(release_age as f64 / RELEASE_FRAMES as f64)
    };
    let phase = TAU * 440.0 * age as f64 / f64::from(RATE);
    ((phase.sin() as f32) as f64 * amplitude * f64::from(carrier)) as f32
}

fn expected_at(frame: u64, carrier: f32) -> f32 {
    let mut sample = 0.0;
    for start in [0, 24_000] {
        if frame >= start {
            sample += expected_note(frame - start, 6_000, carrier);
        }
    }
    sample
}

fn assert_independent_pcm(frames: &[[f32; 2]], first_frame: u64, carrier: f32) {
    for (offset, frame) in frames.iter().enumerate() {
        let age = first_frame + offset as u64;
        let expected = expected_at(age, carrier);
        assert!(
            (frame[0] - expected).abs() < 0.000_02,
            "Operator PCM mismatch at frame {age}: got {}, expected {expected}",
            frame[0]
        );
        assert_eq!(frame[0], frame[1], "single carrier is centered mono");
    }
}

#[test]
fn registered_operator_matches_independent_arrangement_session_seek_and_loop_pcm() {
    let mut fixture = fixture();
    let revision = prepared(&fixture, None);
    assert_eq!(
        hex_engine::native_device_catalog_for_class(hex_engine::NativeDeviceClass::Instrument)
            .expect("instrument catalog")
            .iter()
            .find(|entry| entry.type_id == OPERATOR)
            .expect("Operator catalog entry")
            .parameters
            .len(),
        69
    );

    let mut renderer = Renderer::new(revision.arrangement());
    renderer.play();
    let mut arrangement = vec![[0.0; 2]; 7_000];
    renderer
        .render(&mut arrangement)
        .expect("Arrangement render");
    assert_independent_pcm(&arrangement, 0, 1.0);

    let mut session_runtime = SessionPlayback::new(Arc::clone(&revision)).expect("runtime");
    session_runtime
        .apply(SessionPlaybackCommand::Play)
        .expect("Arrangement transport");
    let mut session_arrangement = vec![[0.0; 2]; arrangement.len()];
    session_runtime
        .render(&mut session_arrangement)
        .expect("Session-backed Arrangement render");
    assert_eq!(session_arrangement, arrangement);

    let mut sought = SessionPlayback::new(Arc::clone(&revision)).expect("runtime");
    sought
        .apply(SessionPlaybackCommand::Seek(5_000))
        .expect("seek into the first Operator note");
    sought
        .apply(SessionPlaybackCommand::Play)
        .expect("resume after seek");
    let mut sought_pcm = [[0.0; 2]; 512];
    sought.render(&mut sought_pcm).expect("seek render");
    assert_independent_pcm(&sought_pcm, 5_000, 1.0);

    fixture
        .editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: LOOP_FRAMES,
            enabled: true,
        })
        .expect("set finite Arrangement loop");
    let looped_revision = prepared(&fixture, None);
    let mut looped = SessionPlayback::new(looped_revision).expect("loop runtime");
    looped
        .apply(SessionPlaybackCommand::Play)
        .expect("play looped Arrangement");
    let mut loop_pcm = vec![[0.0; 2]; 12_500];
    looped.render(&mut loop_pcm).expect("loop render");
    for (frame_index, frame) in loop_pcm.iter().enumerate() {
        let age = (frame_index as u64) % LOOP_FRAMES;
        let expected = expected_note(age, 6_000, 1.0);
        assert!((frame[0] - expected).abs() < 0.000_02);
        assert_eq!(frame[0], frame[1]);
    }

    // The Session fixture owns the copied source without an overlapping
    // Arrangement owner at frame zero, so this is an independent single-note
    // oracle rather than a route-transition overlap test.
    fixture
        .editor
        .execute(Command::RemoveMidiClip(fixture.arrangement_clip))
        .expect("remove Arrangement source after its independent render checks");
    let session_revision = prepared(&fixture, Some(revision.session()));
    let mut session = SessionPlayback::new(Arc::clone(&session_revision)).expect("runtime");
    session
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .expect("launch copied Operator MIDI source");
    session
        .apply(SessionPlaybackCommand::Play)
        .expect("Session transport");
    let mut session_pcm = vec![[0.0; 2]; arrangement.len()];
    session.render(&mut session_pcm).expect("Session render");
    assert_independent_pcm(&session_pcm, 0, 1.0);
}

#[test]
fn changed_operator_context_keeps_old_gate_and_uses_new_law_for_future_notes() {
    let mut fixture = fixture();
    let old_revision = prepared(&fixture, None);
    let mut old = SessionPlayback::new(Arc::clone(&old_revision)).expect("old runtime");
    old.apply(SessionPlaybackCommand::Play)
        .expect("play old Operator revision");
    let mut prefix = [[0.0; 2]; 1_000];
    old.render(&mut prefix).expect("old prefix");
    assert_independent_pcm(&prefix, 0, 1.0);

    let current_state = fixture
        .editor
        .project()
        .track(fixture.track)
        .and_then(|track| track.instrument())
        .expect("Operator slot")
        .state()
        .clone();
    let changed = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        &current_state,
        ParameterId::from_static(29),
        ParameterValue::Float(0.5),
    )
    .expect("change Operator carrier");
    fixture
        .editor
        .execute(Command::ReplaceDeviceState {
            device: fixture.instrument,
            state: changed.clone(),
        })
        .expect("publish changed Operator state");
    assert!(
        fixture.editor.undo(),
        "Operator state edit has one undo step"
    );
    assert_ne!(
        fixture
            .editor
            .project()
            .device(fixture.instrument)
            .expect("instrument after undo")
            .state(),
        &changed
    );
    assert!(
        fixture.editor.redo(),
        "Operator state edit has one redo step"
    );
    assert_eq!(
        fixture
            .editor
            .project()
            .device(fixture.instrument)
            .expect("instrument after redo")
            .state(),
        &changed
    );
    let candidate_revision = prepared(&fixture, Some(old_revision.session()));
    let mut candidate = SessionPlayback::new(candidate_revision).expect("candidate runtime");
    candidate
        .try_inherit_state_from(&mut old)
        .expect("compatible changed-context handoff");

    let mut absolute_frame = prefix.len() as u64;
    let mut output = vec![[0.0; 2]; 26_000 - prefix.len()];
    for part in output.chunks_mut(257) {
        candidate
            .render(part)
            .expect("render after context handoff");
        for (offset, frame) in part.iter().enumerate() {
            let frame_index = absolute_frame + offset as u64;
            let first = expected_note(frame_index, 6_000, 1.0);
            let second = if frame_index >= 24_000 {
                expected_note(frame_index - 24_000, 6_000, 0.5)
            } else {
                0.0
            };
            let expected = first + second;
            assert!(
                (frame[0] - expected).abs() < 0.000_02,
                "old/new Operator context mismatch at frame {frame_index}: got {}, expected {expected}",
                frame[0]
            );
            assert_eq!(frame[0], frame[1]);
        }
        absolute_frame += part.len() as u64;
    }
}
