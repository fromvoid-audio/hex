#[path = "../../hex-devices/tests/support/mod.rs"]
mod support;
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
fn lfo_arrangement_session_partitions_seek_and_loop_share_the_prepared_law() {
    for (sync, selected) in [false, true]
        .into_iter()
        .flat_map(|sync| [false, true].map(|selected| (sync, selected)))
    {
        let mut fixture = fixture();
        let mut state = operator_state(1.0);
        for (id, value) in [
            (82, ParameterValue::Bool(true)),
            (84, ParameterValue::Bool(sync)),
            (85, ParameterValue::Float(7.0)),
            (87, ParameterValue::Float(75.0)),
            (89, ParameterValue::Bool(selected)),
        ] {
            state = hex_engine::patch_native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                &state,
                ParameterId::from_static(id),
                value,
            )
            .unwrap();
        }
        fixture
            .editor
            .execute(Command::ReplaceDeviceState {
                device: fixture.instrument,
                state,
            })
            .unwrap();
        let revision = prepared(&fixture, None);
        let mut arrangement = start_route(Arc::clone(&revision), None);
        let mut session = start_route(Arc::clone(&revision), Some(fixture.session_clip));
        let mut expected = vec![[0.0; 2]; 7000];
        let mut actual = expected.clone();
        arrangement.render(&mut expected).unwrap();
        for block in actual.chunks_mut(113) {
            session.render(block).unwrap();
        }
        assert_eq!(actual, expected, "Session vs Arrangement sync={sync}");
        let mut phase = 0.0;
        for (index, frame) in expected.iter().enumerate().take(6000) {
            let sample = (TAU * phase).sin() as f32;
            assert!(
                (frame[0] - sample).abs() < 3e-6,
                "host clock oracle sync={sync} at {index}"
            );
            let lfo =
                (TAU * index as f64 * if sync { 2.0 / 48_000.0 } else { 7.0 / 48_000.0 }).sin();
            phase += 440.0 / 48_000.0 * if selected { (0.75 * lfo).exp2() } else { 1.0 };
        }
        let mut sought = start_route(Arc::clone(&revision), None);
        assert!(
            sought.apply(SessionPlaybackCommand::Seek(3000)).is_err(),
            "held mutable seek requires control-prepared preroll"
        );
        assert_eq!(sought.performance_frame(), 0);
        let mut copy = start_route(Arc::clone(&revision), None);
        let mut prefix = [[0.0; 2]; 173];
        copy.render(&mut prefix).unwrap();
        let mut replacement = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        replacement.try_inherit_state_from(&mut copy).unwrap();
        copy = replacement;
        let mut continued = [[0.0; 2]; 128];
        copy.render(&mut continued).unwrap();
        assert_eq!(&continued[..], &expected[173..301]);
        fixture
            .editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: LOOP_FRAMES,
                enabled: true,
            })
            .unwrap();
        let looped_revision = prepared(&fixture, None);
        let mut looped = start_route(looped_revision, None);
        let mut loop_pcm = vec![[0.0; 2]; 12_500];
        looped.render(&mut loop_pcm).unwrap();
        assert_eq!(
            &loop_pcm[12_000..],
            &expected[..500],
            "note retrigger at loop onset"
        );
    }
}

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
    let mut state = support::legacy_operator_state();
    for (parameter, value) in [
        (29, carrier), // op1 is the only default carrier
        (33, 1.0),     // initial
        (34, 1.0),     // peak
        (35, 1.0),     // sustain
        (36, 0.0),     // attack
        (37, 0.001),   // minimum decay; peak equals sustain, so PCM is unchanged
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
        .execute(Command::InsertTrackWithDevice {
            name: "operator".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
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

fn edit_feedback(fixture: &mut Fixture, amount: f32) {
    let current = fixture
        .editor
        .project()
        .track(fixture.track)
        .and_then(|track| track.instrument())
        .expect("Operator slot")
        .state();
    let state = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        current,
        ParameterId::from_static(78),
        ParameterValue::Float(amount),
    )
    .expect("feedback patch");
    fixture
        .editor
        .execute(Command::ReplaceDeviceState {
            device: fixture.instrument,
            state,
        })
        .expect("edit Operator feedback");
}

fn start_route(
    revision: Arc<PreparedRevision>,
    session_clip: Option<hex_project::ClipId>,
) -> SessionPlayback {
    let mut playback = SessionPlayback::new(revision).expect("runtime");
    if let Some(clip) = session_clip {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: hex_engine::LaunchQuantization::Immediate,
            })
            .expect("launch Session clip");
    }
    playback
        .apply(SessionPlaybackCommand::Play)
        .expect("play route");
    playback
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
    let catalog =
        hex_engine::native_device_catalog_for_class(hex_engine::NativeDeviceClass::Instrument)
            .expect("instrument catalog");
    let operator = catalog
        .iter()
        .find(|entry| entry.type_id == OPERATOR)
        .expect("Operator catalog entry");
    assert_eq!(operator.parameters.len(), 428);
    let enables: Vec<_> = operator
        .parameters
        .iter()
        .filter(|parameter| (513..=516).contains(&parameter.id.value()))
        .map(|parameter| (parameter.id.value(), parameter.kind, parameter.default))
        .collect();
    assert_eq!(
        enables,
        (513..=516)
            .map(|id| (
                id,
                hex_lattice_contract::ParameterKind::Bool,
                ParameterValue::Bool(true)
            ))
            .collect::<Vec<_>>()
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

#[test]
fn operator_feedback_class_changes_keep_old_audio_while_canonical_state_changes() {
    fn feedback_state(fixture: &Fixture, amount: f32) -> hex_lattice_contract::NativeDeviceState {
        let current = fixture
            .editor
            .project()
            .track(fixture.track)
            .and_then(|track| track.instrument())
            .expect("Operator slot")
            .state();
        hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            current,
            ParameterId::from_static(78),
            ParameterValue::Float(amount),
        )
        .expect("feedback patch")
    }

    // Arrangement: an active mutable occurrence cannot be silently replaced
    // by the analytic class on the same stable device identity.
    let mut arrangement_fixture = fixture();
    let feedback = feedback_state(&arrangement_fixture, 0.45);
    arrangement_fixture
        .editor
        .execute(Command::ReplaceDeviceState {
            device: arrangement_fixture.instrument,
            state: feedback,
        })
        .expect("enable native feedback");
    let mutable_revision = prepared(&arrangement_fixture, None);
    let mut active = SessionPlayback::new(Arc::clone(&mutable_revision)).expect("active");
    let mut reference = SessionPlayback::new(Arc::clone(&mutable_revision)).expect("reference");
    for playback in [&mut active, &mut reference] {
        playback.apply(SessionPlaybackCommand::Play).expect("play");
    }
    let mut prefix = [[0.0; 2]; 64];
    let mut reference_prefix = [[0.0; 2]; 64];
    active.render(&mut prefix).expect("active prefix");
    reference
        .render(&mut reference_prefix)
        .expect("reference prefix");
    assert_eq!(prefix, reference_prefix);
    assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));

    let analytic = feedback_state(&arrangement_fixture, 0.0);
    arrangement_fixture
        .editor
        .execute(Command::ReplaceDeviceState {
            device: arrangement_fixture.instrument,
            state: analytic,
        })
        .expect("disable feedback on the same device");
    let canonical_state = arrangement_fixture
        .editor
        .project()
        .track(arrangement_fixture.track)
        .and_then(|track| track.instrument())
        .expect("canonical Operator slot")
        .state();
    assert_eq!(
        hex_engine::native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            canonical_state,
            ParameterId::from_static(78),
        )
        .expect("canonical feedback value"),
        ParameterValue::Float(0.0),
        "the desired edit is committed even when active-audio handoff refuses"
    );
    let analytic_revision = prepared(&arrangement_fixture, Some(mutable_revision.session()));
    let mut refused = SessionPlayback::new(analytic_revision).expect("candidate");
    let before = active.performance_frame();
    assert_eq!(
        refused.try_inherit_state_from(&mut active),
        Err(hex_engine::SessionPlaybackError::SourceHistoryUnavailable)
    );
    assert_eq!(active.performance_frame(), before);
    let mut after_refusal = [[0.0; 2]; 64];
    let mut expected = [[0.0; 2]; 64];
    active.render(&mut after_refusal).expect("old graph retry");
    reference
        .render(&mut expected)
        .expect("reference continuation");
    assert_eq!(after_refusal, expected);

    // Session route: the inverse analytic-to-mutable class change is also
    // refused while its copied MIDI occurrence is sounding.
    let mut session_fixture = fixture();
    let analytic_revision = prepared(&session_fixture, None);
    let mut session_active =
        SessionPlayback::new(Arc::clone(&analytic_revision)).expect("analytic Session runtime");
    let mut session_reference =
        SessionPlayback::new(Arc::clone(&analytic_revision)).expect("reference Session runtime");
    session_active
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: session_fixture.session_clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .expect("launch Session clip");
    session_reference
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: session_fixture.session_clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .expect("launch reference Session clip");
    session_active
        .apply(SessionPlaybackCommand::Play)
        .expect("play Session route");
    session_reference
        .apply(SessionPlaybackCommand::Play)
        .expect("play reference Session route");
    let mut session_prefix = [[0.0; 2]; 64];
    let mut reference_prefix = [[0.0; 2]; 64];
    session_active
        .render(&mut session_prefix)
        .expect("active Session prefix");
    session_reference
        .render(&mut reference_prefix)
        .expect("reference Session prefix");
    assert_eq!(session_prefix, reference_prefix);
    assert!(session_prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));

    let feedback = feedback_state(&session_fixture, 0.45);
    session_fixture
        .editor
        .execute(Command::ReplaceDeviceState {
            device: session_fixture.instrument,
            state: feedback,
        })
        .expect("enable feedback on same Session instrument");
    let canonical_state = session_fixture
        .editor
        .project()
        .track(session_fixture.track)
        .and_then(|track| track.instrument())
        .expect("canonical Session Operator slot")
        .state();
    assert_eq!(
        hex_engine::native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            canonical_state,
            ParameterId::from_static(78),
        )
        .expect("canonical feedback value"),
        ParameterValue::Float(0.45),
        "the desired Session edit remains committed after active-audio handoff refusal"
    );
    let feedback_revision = prepared(&session_fixture, Some(analytic_revision.session()));
    let mut session_candidate = SessionPlayback::new(feedback_revision).expect("candidate");
    let before = session_active.performance_frame();
    assert_eq!(
        session_candidate.try_inherit_state_from(&mut session_active),
        Err(hex_engine::SessionPlaybackError::MutableInstrumentChase)
    );
    assert_eq!(session_active.performance_frame(), before);
    let mut after_refusal = [[0.0; 2]; 64];
    let mut expected = [[0.0; 2]; 64];
    session_active
        .render(&mut after_refusal)
        .expect("old Session graph retry");
    session_reference
        .render(&mut expected)
        .expect("reference Session continuation");
    assert_eq!(after_refusal, expected);
}

#[test]
fn operator_feedback_class_change_refusal_covers_other_route_directions() {
    for (session_route, old_feedback) in [(false, false), (true, true)] {
        let mut fixture = fixture();
        if old_feedback {
            edit_feedback(&mut fixture, 0.45);
        }
        let old_revision = prepared(&fixture, None);
        let route_clip = session_route.then_some(fixture.session_clip);
        let mut active = start_route(Arc::clone(&old_revision), route_clip);
        let mut reference = start_route(Arc::clone(&old_revision), route_clip);
        let mut prefix = [[0.0; 2]; 64];
        let mut reference_prefix = [[0.0; 2]; 64];
        active.render(&mut prefix).expect("active prefix");
        reference
            .render(&mut reference_prefix)
            .expect("reference prefix");
        assert_eq!(prefix, reference_prefix);
        assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));

        edit_feedback(&mut fixture, if old_feedback { 0.0 } else { 0.45 });
        let next_revision = prepared(&fixture, Some(old_revision.session()));
        let mut candidate = SessionPlayback::new(next_revision).expect("candidate");
        let frame_before = active.performance_frame();
        assert_eq!(
            candidate.try_inherit_state_from(&mut active),
            Err(if old_feedback {
                hex_engine::SessionPlaybackError::SourceHistoryUnavailable
            } else {
                hex_engine::SessionPlaybackError::MutableInstrumentChase
            }),
            "route={session_route}, old feedback={old_feedback}"
        );
        assert_eq!(active.performance_frame(), frame_before);

        let mut actual = [[0.0; 2]; 64];
        let mut expected = [[0.0; 2]; 64];
        active.render(&mut actual).expect("old graph continuation");
        reference
            .render(&mut expected)
            .expect("reference continuation");
        assert_eq!(
            actual, expected,
            "route={session_route}, old feedback={old_feedback}"
        );
    }
}

#[test]
fn operator_feedback_exact_cut_note_on_is_admitted_once_on_both_routes() {
    for session_route in [false, true] {
        let mut fixture = fixture();
        let old_revision = prepared(&fixture, None);
        let session_clip = session_route.then_some(fixture.session_clip);
        let mut active = start_route(Arc::clone(&old_revision), session_clip);

        edit_feedback(&mut fixture, 0.45);
        let candidate_revision = prepared(&fixture, Some(old_revision.session()));
        let mut candidate = SessionPlayback::new(Arc::clone(&candidate_revision))
            .expect("feedback candidate at the exact onset cut");
        candidate
            .try_inherit_state_from(&mut active)
            .expect("an onset exactly at the cut belongs to the new graph");

        let mut reference = start_route(candidate_revision, session_clip);
        let mut actual = vec![[0.0; 2]; 30_000];
        let mut expected = vec![[0.0; 2]; 30_000];
        for (actual_block, expected_block) in actual.chunks_mut(127).zip(expected.chunks_mut(127)) {
            candidate
                .render(actual_block)
                .expect("render admitted exact-cut onset");
            reference
                .render(expected_block)
                .expect("render independent feedback reference");
        }
        let mismatch = actual
            .iter()
            .zip(&expected)
            .position(|(actual, expected)| actual != expected);
        assert_eq!(
            mismatch.map(|index| (index, actual[index], expected[index])),
            None,
            "Session route={session_route}"
        );
        assert!(actual.iter().any(|frame| frame[0].abs() > 1.0e-6));
    }
}

#[test]
fn operator_feedback_handoff_at_exclusive_release_end_is_quiescent() {
    let mut fixture = fixture();
    edit_feedback(&mut fixture, 0.45);
    let old_revision = prepared(&fixture, None);
    let mut previous = start_route(Arc::clone(&old_revision), None);
    // The runtime occurrence remains owned through the maximum declared
    // oscillator release, including currently silent oscillators.
    const MAX_OPERATOR_RELEASE_FRAMES: u64 = 4_800;
    let exact_gate_release_end = 6_000 + MAX_OPERATOR_RELEASE_FRAMES;
    let mut prefix = vec![[0.0; 2]; exact_gate_release_end as usize];
    previous
        .render(&mut prefix)
        .expect("render through exclusive end");
    assert_eq!(previous.performance_frame(), exact_gate_release_end);

    edit_feedback(&mut fixture, 0.0);
    let candidate_revision = prepared(&fixture, Some(old_revision.session()));
    let mut candidate = SessionPlayback::new(Arc::clone(&candidate_revision)).expect("candidate");
    candidate
        .try_inherit_state_from(&mut previous)
        .expect("no active mutable occurrence remains at its exclusive release end");
    assert_eq!(candidate.performance_frame(), exact_gate_release_end);

    let mut reference = SessionPlayback::new(candidate_revision).expect("fresh reference");
    reference
        .apply(SessionPlaybackCommand::Seek(exact_gate_release_end))
        .expect("seek before the next note");
    reference
        .apply(SessionPlaybackCommand::Play)
        .expect("play reference");
    let remaining = 25_000 - exact_gate_release_end as usize;
    let mut actual = vec![[0.0; 2]; remaining];
    let mut expected = vec![[0.0; 2]; remaining];
    for (actual_block, expected_block) in actual.chunks_mut(257).zip(expected.chunks_mut(257)) {
        candidate
            .render(actual_block)
            .expect("render transitioned runtime");
        reference
            .render(expected_block)
            .expect("render independent seek reference");
    }
    assert_eq!(actual, expected);
    assert!(actual.iter().any(|frame| frame[0].abs() > 1.0e-6));
}

#[test]
fn operator_source_removal_carries_old_feedback_release_without_retriggering() {
    let mut fixture = fixture();
    edit_feedback(&mut fixture, 0.45);
    let old_revision = prepared(&fixture, None);
    let mut active = start_route(Arc::clone(&old_revision), None);
    let mut reference = start_route(Arc::clone(&old_revision), None);
    let mut prefix = [[0.0; 2]; 64];
    let mut reference_prefix = [[0.0; 2]; 64];
    active.render(&mut prefix).expect("active prefix");
    reference
        .render(&mut reference_prefix)
        .expect("reference prefix");
    assert_eq!(prefix, reference_prefix);
    assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));

    fixture
        .editor
        .execute(Command::RemoveMidiClip(fixture.arrangement_clip))
        .expect("remove active source");
    let empty_revision = prepared(&fixture, Some(old_revision.session()));
    let mut candidate = SessionPlayback::new(empty_revision).expect("source-empty candidate");
    candidate
        .try_inherit_state_from(&mut active)
        .expect("source removal can retain the old feedback occurrence and tail");

    let remaining_frames = 25_000 - 64;
    let mut actual = vec![[0.0; 2]; remaining_frames];
    let mut reference_pcm = vec![[0.0; 2]; remaining_frames];
    for (actual_block, reference_block) in actual.chunks_mut(257).zip(reference_pcm.chunks_mut(257))
    {
        candidate
            .render(actual_block)
            .expect("render retained source tail");
        reference
            .render(reference_block)
            .expect("render unchanged source reference");
    }
    let tail_end_from_start = (6_000 + RELEASE_FRAMES) as usize - 64;
    assert_eq!(
        &actual[..tail_end_from_start],
        &reference_pcm[..tail_end_from_start],
        "old feedback voice keeps its original gate and release"
    );
    assert!(
        actual[tail_end_from_start..]
            .iter()
            .all(|frame| *frame == [0.0; 2]),
        "removed source retriggered a note"
    );
    assert!(
        reference_pcm[24_000 - 64..]
            .iter()
            .any(|frame| frame[0].abs() > 1.0e-6)
    );
}

#[test]
fn operator_session_source_removal_carries_old_feedback_release_without_retriggering() {
    let mut source_fixture = fixture();
    edit_feedback(&mut source_fixture, 0.45);
    let old_revision = prepared(&source_fixture, None);
    let mut active = start_route(Arc::clone(&old_revision), Some(source_fixture.session_clip));
    let mut reference = start_route(Arc::clone(&old_revision), Some(source_fixture.session_clip));
    let mut prefix = [[0.0; 2]; 64];
    let mut reference_prefix = [[0.0; 2]; 64];
    active.render(&mut prefix).expect("active Session prefix");
    reference
        .render(&mut reference_prefix)
        .expect("reference Session prefix");
    assert_eq!(prefix, reference_prefix);
    assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));

    let notes = source_fixture
        .editor
        .project()
        .midi_source(source_fixture.session_clip)
        .expect("Session source")
        .notes()
        .iter()
        .map(|note| note.id())
        .collect();
    source_fixture
        .editor
        .execute(Command::RemoveMidiNotes {
            clip: source_fixture.session_clip,
            notes,
        })
        .expect("remove Session source notes");
    let source_empty_revision = prepared(&source_fixture, Some(old_revision.session()));
    let mut candidate =
        SessionPlayback::new(source_empty_revision).expect("source-empty candidate");
    candidate
        .try_inherit_state_from(&mut active)
        .expect("Session source removal can retain the old feedback occurrence and tail");

    let remaining_frames = 50_000 - 64;
    let mut actual = vec![[0.0; 2]; remaining_frames];
    let mut reference_pcm = vec![[0.0; 2]; remaining_frames];
    for (actual_block, reference_block) in actual.chunks_mut(257).zip(reference_pcm.chunks_mut(257))
    {
        candidate
            .render(actual_block)
            .expect("render retained Session tail");
        reference
            .render(reference_block)
            .expect("render unchanged Session source reference");
    }
    assert_eq!(
        &actual[..(6_000 + RELEASE_FRAMES) as usize - 64],
        &reference_pcm[..(6_000 + RELEASE_FRAMES) as usize - 64],
        "the already-active Session note keeps its authored gate and feedback release"
    );
    assert!(
        actual[(6_000 + RELEASE_FRAMES) as usize - 64..]
            .iter()
            .all(|frame| *frame == [0.0; 2]),
        "removed Session source retriggered a note"
    );
    assert!(
        reference_pcm[24_000 - 64..]
            .iter()
            .any(|frame| frame[0].abs() > 1.0e-6),
        "the unedited reference includes the later authored Session note"
    );
}

#[test]
fn registered_operator_feedback_wraps_with_partition_independent_attack() {
    let mut fixture = fixture();
    edit_feedback(&mut fixture, 0.45);
    fixture
        .editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 12_000,
            enabled: true,
        })
        .expect("enable Arrangement transport loop");
    let revision = prepared(&fixture, None);
    assert!(revision.has_arrangement_mutable_instruments());

    fn render(revision: Arc<PreparedRevision>, partition: usize) -> Vec<[f32; 2]> {
        let mut playback = SessionPlayback::new(revision).expect("feedback loop runtime");
        playback
            .apply(SessionPlaybackCommand::Play)
            .expect("play looped Operator");
        let mut output = vec![[0.0; 2]; 25_000];
        for block in output.chunks_mut(partition) {
            playback.render(block).expect("render feedback loop");
        }
        output
    }

    let expected = render(Arc::clone(&revision), 17);
    assert_eq!(render(revision, 257), expected);
    assert!(
        expected[12_000..12_128]
            .iter()
            .any(|frame| frame[0].abs() > 1.0e-6)
    );
    assert!(
        expected[24_000..]
            .iter()
            .any(|frame| frame[0].abs() > 1.0e-6)
    );
}
