//! Independent rendered-pitch coverage for the actual registered generators.
use hex_engine::{
    LaunchQuantization, PlaybackAction, PreparedRevision, SessionPlaybackCommand,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState, ParameterValue};
use hex_project::{BeatTime, ClipId, Command, Created, Editor, Project};
use hex_tuning::{Audition, MAX_DEGREES, ProjectTuning, Scale};
use std::{collections::BTreeMap, sync::Arc};

const RATE: u32 = 48_000;
const KINDS: [DeviceTypeId; 3] = [
    hex_devices::SINE_INSTRUMENT_TYPE_ID,
    hex_devices::fm4::device::FM4_TYPE_ID,
    hex_devices::operator_state::OPERATOR_TYPE_ID,
];

fn tone_state(kind: DeviceTypeId) -> NativeDeviceState {
    let registry = hex_devices::native_instrument_registry().unwrap();
    let mut state = registry.default_state(kind).unwrap();
    if kind == hex_devices::SINE_INSTRUMENT_TYPE_ID {
        return state;
    }
    use hex_devices::fm4::device::parameter as p;
    // Single carrier, no FM: frequency is independently observable rather than
    // inferred by comparing two users of the same preparation implementation.
    for operator in 1..=4 {
        for (id, value) in [
            (
                p::oscillator_level(operator),
                if operator == 1 { 1.0 } else { 0.0 },
            ),
            (p::carrier(operator), if operator == 1 { 1.0 } else { 0.0 }),
        ] {
            state = registry
                .patch_parameter(&state, id, ParameterValue::Float(value))
                .unwrap();
        }
        for target in 1..=4 {
            if target != operator {
                state = registry
                    .patch_parameter(
                        &state,
                        p::modulation(operator, target),
                        ParameterValue::Float(0.0),
                    )
                    .unwrap();
            }
        }
    }
    if kind == hex_devices::fm4::device::FM4_TYPE_ID {
        for (id, value) in [(p::ATTACK, 0.0), (p::DECAY, 0.0), (p::SUSTAIN, 1.0)] {
            state = registry
                .patch_parameter(&state, id, ParameterValue::Float(value))
                .unwrap();
        }
    } else {
        use hex_devices::operator_state::parameter as v;
        use v::EnvelopeField::*;
        // Retain the current Vector processing mode and its default quality,
        // rather than forcing the legacy renderer to make the oracle pass.
        assert_eq!(
            registry
                .definition(kind)
                .unwrap()
                .read_parameter(&state, v::GLOBAL_MODE)
                .unwrap(),
            ParameterValue::Choice(1)
        );
        for field in [Initial, Peak, Sustain, Attack, Decay] {
            let value = if matches!(field, Initial | Peak | Sustain) {
                1.0
            } else if matches!(field, Decay) {
                0.001
            } else {
                0.0
            };
            state = registry
                .patch_parameter(&state, v::envelope(1, field), ParameterValue::Float(value))
                .unwrap();
        }
    }
    state
}

fn unequal_tuning(tritave: bool) -> (ProjectTuning, [f32; MAX_DEGREES], usize, f32) {
    let degrees: &[f32] = if tritave {
        &[0.0, 177.0, 393.0, 786.0, 1270.0]
    } else {
        &[0.0, 80.0, 190.0, 340.0, 510.0, 690.0, 800.0]
    };
    let period = if tritave { 1901.955_f32 } else { 900.0_f32 };
    let mut cents = [0.0; MAX_DEGREES];
    cents[..degrees.len()].copy_from_slice(degrees);
    let scale = Scale::new(degrees.len(), cents, 64, 337.45, 2, period).unwrap();
    (
        ProjectTuning::new(true, scale, Audition::default()).unwrap(),
        cents,
        degrees.len(),
        period,
    )
}

fn expected_hz(key: u8, cents: &[f32; MAX_DEGREES], count: usize, period: f32) -> f64 {
    let degree = i32::from(key) - 64 + 2;
    let mapped = f64::from(cents[degree.rem_euclid(count as i32) as usize])
        + f64::from(period) * f64::from(degree.div_euclid(count as i32))
        - f64::from(cents[2]);
    337.45 * 2.0_f64.powf(mapped / 1200.0)
}

fn project(kind: DeviceTypeId, pitch: u8, tuning: ProjectTuning) -> (Project, ClipId) {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "tuning oracle".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    // Explicit fixture construction also works after the separate empty-new-track
    // policy: it never depends on an implicitly seeded sine instrument.
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
            state: tone_state(kind),
        })
        .unwrap();
    editor.execute(Command::SetTuning(tuning)).unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "tuning oracle".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(session) = editor
        .execute(Command::CopyArrangementMidiToSession {
            source: clip,
            scene,
        })
        .unwrap()
    else {
        panic!("session")
    };
    (editor.project().clone(), session)
}

fn prepare(project: &Project, previous: Option<&PreparedRevision>) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(
            project,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            previous.map(|old| old.session().as_ref()),
        )
        .unwrap(),
    )
}
fn render(
    revision: Arc<PreparedRevision>,
    clip: ClipId,
    session: bool,
    frames: usize,
) -> Vec<[f32; 2]> {
    let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
    if session {
        playback
            .try_apply_action(PlaybackAction::LaunchAndPlay(
                SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
    } else {
        playback.apply(PlaybackCommand::Play);
    }
    let mut out = vec![[0.0; 2]; frames];
    for block in out.chunks_mut(137) {
        playback.render(block).unwrap();
    }
    out
}
fn measured_hz(frames: &[[f32; 2]]) -> f64 {
    // Positive zero crossings with linear interpolation, independent of engine
    // phase steps, provider functions, amplitude and constant filter phase delay.
    let mut crossings = Vec::new();
    for (index, pair) in frames.windows(2).enumerate().skip(960) {
        assert!(pair.iter().flatten().all(|v| v.is_finite()));
        if pair[0][0] <= 0.0 && pair[1][0] > 0.0 {
            let a = f64::from(pair[0][0]);
            let b = f64::from(pair[1][0]);
            crossings.push(index as f64 - a / (b - a));
        }
    }
    assert!(
        crossings.len() >= 8,
        "audible oracle needs repeated real periods, got {}",
        crossings.len()
    );
    f64::from(RATE) * (crossings.len() - 1) as f64 / (crossings.last().unwrap() - crossings[0])
}

#[test]
fn native_tuning_registered_generators_render_unequal_non_octave_pitch_in_both_hosts() {
    for kind in KINDS {
        for tritave in [false, true] {
            let (tuning, cents, count, period) = unequal_tuning(tritave);
            for key in [60, 64, 69] {
                let hz = expected_hz(key, &cents, count, period);
                let (project, clip) = project(kind, key, tuning);
                let revision = prepare(&project, None);
                for session in [false, true] {
                    let output = render(Arc::clone(&revision), clip, session, 8192);
                    let actual = measured_hz(&output);
                    assert!(
                        (actual - hz).abs() < 0.01,
                        "{kind:?}, {count} degrees/{period}cents, key{key}, session={session}: measured{actual} vs independent{hz}"
                    );
                    assert!(output.iter().any(|f| f[0].abs() > 0.001));
                    if kind == hex_devices::SINE_INSTRUMENT_TYPE_ID {
                        for (age, frame) in output.iter().enumerate() {
                            let expected =
                                (std::f64::consts::TAU * hz * age as f64 / f64::from(RATE)).sin()
                                    * 0.1
                                    * (age as f64 / 240.0).min(1.0);
                            assert!((f64::from(frame[0]) - expected).abs() < 0.000002);
                            assert_eq!(frame[0], frame[1]);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn native_tuning_enabled_neutral_table_keeps_registered_host_pcm_exact() {
    for kind in KINDS {
        let (disabled, clip) = project(kind, 60, ProjectTuning::default());
        let mut snapshot = disabled.snapshot();
        snapshot.tuning = snapshot.tuning.with_enabled(true);
        let enabled = Project::from_snapshot(snapshot).unwrap();
        let before = prepare(&disabled, None);
        let after = prepare(&enabled, Some(&before));
        for session in [false, true] {
            assert_eq!(
                render(Arc::clone(&before), clip, session, 2048),
                render(Arc::clone(&after), clip, session, 2048),
                "neutral enabled intent must preserve {kind:?} PCM exactly"
            );
        }
    }
}

fn with_state(project: Project, state: NativeDeviceState) -> Project {
    let device = project.tracks()[0].instrument().unwrap().id();
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    editor.project().clone()
}

#[test]
fn native_tuning_vector_fixed_hz_is_independent_of_key_and_effective_scale() {
    use hex_devices::operator_state::parameter as p;
    let registry = hex_devices::native_instrument_registry().unwrap();
    let mut state = tone_state(hex_devices::operator_state::OPERATOR_TYPE_ID);
    for (id, value) in [
        (p::fixed(1), ParameterValue::Bool(true)),
        (p::fixed_frequency(1), ParameterValue::Float(487.3)),
        (p::fixed_multiplier(1), ParameterValue::Float(0.75)),
    ] {
        state = registry.patch_parameter(&state, id, value).unwrap();
    }
    let hz = f64::from(487.3_f32) * f64::from(0.75_f32);
    let mut reference = None;
    for tritave in [false, true] {
        for pitch in [60, 69] {
            let (project, clip) = project(
                hex_devices::operator_state::OPERATOR_TYPE_ID,
                pitch,
                unequal_tuning(tritave).0,
            );
            let project = with_state(project, state.clone());
            let revision = prepare(&project, None);
            for session in [false, true] {
                let pcm = render(Arc::clone(&revision), clip, session, 8192);
                assert!((measured_hz(&pcm) - hz).abs() < 0.01);
                if let Some(reference) = &reference {
                    assert_eq!(
                        &pcm, reference,
                        "fixed-only PCM must not follow the mapped note"
                    );
                } else {
                    reference = Some(pcm);
                }
            }
        }
    }
}

fn tone_amplitude(frames: &[[f32; 2]], hz: f64) -> f64 {
    let frames = &frames[960..];
    let mut real = 0.0;
    let mut imag = 0.0;
    let mut weight = 0.0;
    for (age, frame) in frames.iter().enumerate() {
        let window =
            0.5 - 0.5 * (std::f64::consts::TAU * age as f64 / (frames.len() - 1) as f64).cos();
        let phase = std::f64::consts::TAU * hz * age as f64 / f64::from(RATE);
        real += f64::from(frame[0]) * window * phase.cos();
        imag += f64::from(frame[0]) * window * phase.sin();
        weight += window;
    }
    2.0 * real.hypot(imag) / weight
}

#[test]
fn native_tuning_vector_mixed_ratio_and_fixed_carriers_keep_both_declared_pitch_laws() {
    use hex_devices::operator_state::parameter as p;
    let registry = hex_devices::native_instrument_registry().unwrap();
    let mut state = tone_state(hex_devices::operator_state::OPERATOR_TYPE_ID);
    for (id, value) in [
        (p::carrier(1), ParameterValue::Float(0.5)),
        (p::oscillator_level(2), ParameterValue::Float(1.0)),
        (p::carrier(2), ParameterValue::Float(0.5)),
        (p::fixed(2), ParameterValue::Bool(true)),
        (p::fixed_frequency(2), ParameterValue::Float(487.3)),
        (p::fixed_multiplier(2), ParameterValue::Float(0.75)),
    ] {
        state = registry.patch_parameter(&state, id, value).unwrap();
    }
    let fixed = f64::from(487.3_f32) * 0.75;
    for tritave in [false, true] {
        let (tuning, cents, count, period) = unequal_tuning(tritave);
        let ratio = expected_hz(69, &cents, count, period);
        let (project, clip) = project(hex_devices::operator_state::OPERATOR_TYPE_ID, 69, tuning);
        let project = with_state(project, state.clone());
        let revision = prepare(&project, None);
        for session in [false, true] {
            let pcm = render(Arc::clone(&revision), clip, session, 16384);
            assert!(
                tone_amplitude(&pcm, ratio) > 0.1,
                "ratio carrier must use independently mapped {ratio}hz"
            );
            assert!(
                tone_amplitude(&pcm, fixed) > 0.1,
                "fixed carrier must retain {fixed}hz"
            );
            assert!(
                tone_amplitude(&pcm, 440.0) < 0.005,
                "no silent 12tet carrier substitution"
            );
            assert!(
                tone_amplitude(&pcm, ratio * 2.0) < 0.005,
                "fixed op2 must not become its note ratio"
            );
        }
    }
}

#[test]
fn native_tuning_prepared_cache_reuse_keeps_old_pcm_and_retuned_sources_isolated() {
    for kind in KINDS {
        let (a, clip) = project(kind, 69, unequal_tuning(true).0);
        let mut snapshot = a.snapshot();
        snapshot.tuning = unequal_tuning(false).0;
        let b = Project::from_snapshot(snapshot).unwrap();
        let first = prepare(&a, None);
        let before = render(Arc::clone(&first), clip, false, 8192);
        let second = prepare(&b, Some(&first));
        let back = prepare(&a, Some(&second));
        let (.., cents, count, period) = unequal_tuning(false);
        for session in [false, true] {
            let new = render(Arc::clone(&second), clip, session, 8192);
            assert!((measured_hz(&new) - expected_hz(69, &cents, count, period)).abs() < 0.01);
            assert_eq!(
                render(Arc::clone(&first), clip, session, 8192),
                before,
                "preparing another tuning cannot mutate the prior immutable source"
            );
            assert_eq!(
                render(Arc::clone(&back), clip, session, 8192),
                before,
                "restored scale must not reuse stale retuned coefficients"
            );
        }
    }
}

#[test]
fn native_tuning_live_keyboard_uses_the_same_effective_non_octave_table() {
    for kind in KINDS {
        for tritave in [false, true] {
            let (tuning, cents, count, period) = unequal_tuning(tritave);
            let (project, _) = project(kind, 69, tuning);
            let track = project.tracks()[0].id();
            let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
            input.set_key(0, Some((69, 127, 1))).unwrap();
            let mut playback = Playback::from_prepared(prepare(&project, None));
            playback
                .prepare_computer_keyboard(&project, track, input)
                .unwrap();
            // Arrangement is deliberately paused: this measures only the actual
            // selected-track live input path, not a second concurrent clip tone.
            let mut pcm = vec![[0.0; 2]; 8192];
            for block in pcm.chunks_mut(137) {
                playback.render(block).unwrap();
            }
            assert!(
                (measured_hz(&pcm) - expected_hz(69, &cents, count, period)).abs() < 0.01,
                "{kind:?} keyboard must consume the effective table without MTS"
            );
        }
    }
}

#[test]
fn native_tuning_held_and_released_live_voices_retarget_unequal_maps_without_restarting() {
    for kind in KINDS {
        for released in [false, true] {
            let (project, _) = project(kind, 69, unequal_tuning(true).0);
            // A real tuning edit retains copy-on-write MIDI source identity.
            // Snapshot reconstruction is an Open/fresh source, not continuation.
            let mut editor = Editor::new(project.clone(), 8).unwrap();
            editor
                .execute(Command::SetTuning(unequal_tuning(false).0))
                .unwrap();
            let target = editor.project().clone();
            let first = prepare(&project, None);
            let next = Arc::new(
                PreparedRevision::prepare_tuning_from(
                    &target,
                    &BTreeMap::new(),
                    Arc::new(hex_devices::SineTable::new()),
                    &project,
                    &first,
                )
                .unwrap_or_else(|error| {
                    panic!("tuning-only preparation: {kind:?}, released={released}: {error:?}")
                }),
            );
            let track = project.tracks()[0].id();
            let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
            input.set_key(0, Some((69, 127, 1))).unwrap();
            let mut old = Playback::from_prepared(first);
            old.prepare_computer_keyboard(&project, track, Arc::clone(&input))
                .unwrap();
            old.render(&mut vec![[0.0; 2]; 2048]).unwrap();
            let age = if released {
                input.set_key(0, None).unwrap();
                old.render(&mut vec![[0.0; 2]; 137]).unwrap();
                2185
            } else {
                2048
            };
            let mut actual = Playback::from_prepared(next);
            actual
                .prepare_computer_keyboard_for_tuning(
                    &target,
                    track,
                    Arc::clone(&input),
                    &old.computer_keyboard_tuning_hint().unwrap(),
                )
                .unwrap();
            actual.try_inherit_runtime_from(&mut old).unwrap();
            old.cancel_computer_keyboard();
            drop(old);
            assert!(
                !input.is_cancelled(),
                "retired donor cannot cancel transferred input"
            );
            let mut pcm = vec![[0.0; 2]; if released { 2048 } else { 8192 }];
            for block in pcm.chunks_mut(137) {
                actual.render(block).unwrap();
            }
            let (_, cents, count, period) = unequal_tuning(false);
            let hz = expected_hz(69, &cents, count, period);
            assert!(
                (measured_hz(&pcm) - hz).abs() < 0.01,
                "{kind:?} released={released}: carried voice must use new mapped pitch"
            );
            if kind == hex_devices::SINE_INSTRUMENT_TYPE_ID {
                let (_, a_cents, a_count, a_period) = unequal_tuning(true);
                let prior_hz = expected_hz(69, &a_cents, a_count, a_period);
                for (offset, frame) in pcm.iter().enumerate() {
                    let phase = std::f64::consts::TAU
                        * (prior_hz * age as f64 + hz * offset as f64)
                        / f64::from(RATE);
                    let envelope = if released {
                        1.0 - (age + offset - 2048) as f64 / 2400.0
                    } else {
                        1.0
                    };
                    let expected = phase.sin() * 0.1 * envelope;
                    assert!(
                        (f64::from(frame[0]) - expected).abs() < 0.000002,
                        "independent phase/envelope oracle: age{age}+{offset}"
                    );
                }
            }
        }
    }
}

#[test]
fn native_tuning_fresh_or_ordinary_edit_sources_cannot_rebind_a_keyboard_family() {
    for kind in KINDS {
        let (project, _) = project(kind, 69, unequal_tuning(true).0);
        let mut snapshot = project.snapshot();
        snapshot.tuning = unequal_tuning(false).0;
        let target = Project::from_snapshot(snapshot).unwrap();
        let track = project.tracks()[0].id();
        let revision = prepare(&project, None);
        let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((69, 127, 1))).unwrap();
        let mut old = Playback::from_prepared(Arc::clone(&revision));
        old.prepare_computer_keyboard(&project, track, Arc::clone(&input))
            .unwrap();
        old.render(&mut vec![[0.0; 2]; 2048]).unwrap();
        let hint = old.computer_keyboard_tuning_hint().unwrap();
        let mut fresh = Playback::from_prepared(prepare(&target, None));
        assert!(
            fresh
                .prepare_computer_keyboard_for_tuning(&target, track, Arc::clone(&input), &hint)
                .is_err(),
            "fresh source/Open cannot borrow a tuning-only lease"
        );
        let mut editor = Editor::new(target, 8).unwrap();
        editor
            .execute(Command::RenameTrack {
                track,
                name: "ordinary edit".into(),
            })
            .unwrap();
        assert!(
            PreparedRevision::prepare_tuning_from(
                editor.project(),
                &BTreeMap::new(),
                Arc::new(hex_devices::SineTable::new()),
                &project,
                &revision
            )
            .is_err()
        );
        assert!(!input.is_cancelled());
        let mut pcm = vec![[0.0; 2]; 8192];
        old.render(&mut pcm).unwrap();
        let (_, cents, count, period) = unequal_tuning(true);
        assert!(
            (measured_hz(&pcm) - expected_hz(69, &cents, count, period)).abs() < 0.01,
            "refusal must keep the original audible source"
        );
    }
}

#[test]
fn native_tuning_oscillator_ratio_multiplies_the_effective_pitch_in_both_hosts() {
    for kind in [
        hex_devices::fm4::device::FM4_TYPE_ID,
        hex_devices::operator_state::OPERATOR_TYPE_ID,
    ] {
        let (tuning, cents, count, period) = unequal_tuning(false);
        let (project, clip) = project(kind, 69, tuning);
        let track = project.tracks()[0].id();
        let device = project.track(track).unwrap().instrument().unwrap().id();
        let state = hex_devices::native_instrument_registry()
            .unwrap()
            .patch_parameter(
                project.track(track).unwrap().instrument().unwrap().state(),
                hex_devices::fm4::device::parameter::oscillator_ratio(1),
                ParameterValue::Float(2.5),
            )
            .unwrap();
        let mut editor = Editor::new(project, 8).unwrap();
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        let revision = prepare(editor.project(), None);
        for session in [false, true] {
            let pcm = render(Arc::clone(&revision), clip, session, 8192);
            assert!(
                (measured_hz(&pcm) - expected_hz(69, &cents, count, period) * 2.5).abs() < 0.01,
                "{kind:?}: ratio must multiply the effective table, not nominal 12tet"
            );
        }
    }
}

#[test]
fn native_tuning_seek_and_repeated_arrangement_loops_keep_current_non_octave_pitch() {
    for kind in KINDS {
        let (tuning, cents, count, period) = unequal_tuning(true);
        let (project, _) = project(kind, 69, tuning);
        let mut editor = Editor::new(project, 8).unwrap();
        editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 24_000,
                enabled: true,
            })
            .unwrap();
        let revision = prepare(editor.project(), None);
        // Mutable native voices require the real control-prepared seek path;
        // raw callback Seek into a held voice deliberately refuses that work.
        let preroll =
            hex_engine::SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                Arc::clone(&revision),
                2048,
                2048,
                true,
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
        let mut playback = Playback::from_interactive_arrangement_preroll(preroll).unwrap();
        let mut sought = vec![[0.0; 2]; 8192];
        for block in sought.chunks_mut(137) {
            playback.render(block).unwrap();
        }
        let hz = expected_hz(69, &cents, count, period);
        assert!((measured_hz(&sought) - hz).abs() < 0.01, "{kind:?}: seek");
        let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
        playback
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
            .unwrap();
        let mut looped = vec![[0.0; 2]; 58_000];
        for block in looped.chunks_mut(137) {
            playback.render(block).unwrap();
        }
        // The fixture's unchanged default120bpm is 24000 frames per beat.
        // Measure after each actual loop boundary, not a cached pre-loop window.
        for origin in [0, 24_000, 48_000] {
            // Vector deliberately overlaps the prior occurrence's release with
            // its new attack. A changing two-phase sum shifts zero crossings
            // even when both oscillators have the same correct frequency.
            // Measure beyond the fixture's 4800-frame Vector release, without altering
            // the production tail/voice policy or hiding an unsupported route.
            let transition = &looped[origin..origin + 5000];
            assert!(transition.iter().flatten().all(|sample| sample.is_finite()));
            assert!(
                transition
                    .iter()
                    .flatten()
                    .any(|sample| sample.abs() > 0.00001)
            );
            let settled = measured_hz(&looped[origin + 5000..origin + 9000]);
            assert!(
                (settled - hz).abs() < 0.01,
                "{kind:?}: loop at {origin}, measured={settled}, expected={hz}"
            );
        }
    }
}
