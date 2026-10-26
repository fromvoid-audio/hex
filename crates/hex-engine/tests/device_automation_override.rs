//! Shared read selection changes effective values without replacing authored curves.
use hex_engine::{
    Audio, PreparedRevision, Renderer, SessionPlayback, SessionPlaybackCommand, SineTable, compile,
};
use hex_lattice::NativeDeviceDefinition;
use hex_lattice_contract::{
    DeviceAutomationTarget as Target, HostGain, NativeDeviceMetadata, ParameterId, ParameterValue,
};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationReadSelection as Read, AutomationValue,
    BeatTime, Command, Created, DeviceId, Editor, Project,
};
use std::{collections::BTreeMap, sync::Arc};
const RATE: u32 = 8000;
const SIGNAL: [f32; 2] = [0.125, -0.25];
const BLOCK: usize = 64;

struct Fixture {
    editor: Editor,
    device: DeviceId,
    media: BTreeMap<hex_project::AssetId, Audio>,
}
fn fixture(definition: &dyn NativeDeviceDefinition, rate: u32, frames: usize) -> Fixture {
    let mut editor = Editor::new(Project::new(rate).unwrap(), 32).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "override".into(),
            path: "original-override.wav".into(),
            source_sample_rate: rate,
            source_frames: frames as u64,
            playback_frames: frames as u64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: definition.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    Fixture {
        editor,
        device,
        media: BTreeMap::from([(asset, Audio::new(rate, vec![SIGNAL; frames]).unwrap())]),
    }
}
fn point(value: f32) -> AutomationPoint {
    AutomationPoint {
        beat: BeatTime::ZERO,
        value: AutomationValue::new(value).unwrap(),
        outgoing: AutomationInterpolation::Step,
    }
}
fn provider_lane(
    f: &mut Fixture,
    definition: &dyn NativeDeviceDefinition,
    id: ParameterId,
    value: ParameterValue,
) {
    let state = f.editor.project().device(f.device).unwrap().state();
    let descriptor = *definition
        .descriptors()
        .iter()
        .find(|d| d.id == id)
        .unwrap();
    let semantics = definition
        .parameter_semantics_for_state(state, id)
        .unwrap()
        .unwrap();
    let normalized = semantics
        .normalization
        .normalize(descriptor, value)
        .unwrap();
    f.editor
        .execute(Command::ReplaceDeviceAutomation {
            device: f.device,
            parameter: id,
            points: vec![point(normalized)],
        })
        .unwrap();
}
fn revision(f: &Fixture) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    )
}
fn fresh(f: &Fixture, prepared: bool, frames: usize) -> Vec<[f32; 2]> {
    let mut out = vec![[0.0; 2]; frames];
    if prepared {
        let mut runtime = SessionPlayback::new(revision(f)).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        for block in out.chunks_mut(17) {
            runtime.render(block).unwrap();
        }
    } else {
        let graph = compile::project(f.editor.project(), &f.media).unwrap();
        let mut runtime = Renderer::new(&graph);
        runtime.play();
        for block in out.chunks_mut(31) {
            runtime.render(block).unwrap();
        }
    }
    out
}
fn close(actual: &[[f32; 2]], expected: &[[f32; 2]]) {
    assert_eq!(actual.len(), expected.len());
    for (frame, (a, b)) in actual.iter().zip(expected).enumerate() {
        for channel in 0..2 {
            assert!(
                (a[channel] - b[channel]).abs() < 2e-6,
                "frame {frame}: {a:?} != {b:?}"
            );
        }
    }
}
fn oracle(kind: usize, frame: usize, from: f64, to: f64, transition: bool, age: usize) -> [f32; 2] {
    let value = if kind == 2 && transition {
        from + (to - from) * ((age + 1) as f64 / 40.0).min(1.0)
    } else {
        to
    };
    match kind {
        0 => SIGNAL.map(|v| v * value as f32),
        1 => SIGNAL.map(|dry| {
            let x = f64::from(dry) * 10.0_f64.powf(value / 20.0);
            let wet = if x.abs() <= 0.5 {
                x
            } else {
                x.signum() * (1.0 - 0.25 / x.abs())
            };
            (f64::from(dry) * 0.5 + wet * 0.5) as f32
        }),
        _ => std::array::from_fn(|channel| {
            let phase = frame as f64 / f64::from(RATE) + channel as f64 * 0.5;
            let sine = (1.0 + (std::f64::consts::TAU * phase).sin()) * 0.5;
            (f64::from(SIGNAL[channel]) * (1.0 - value + value * sine)) as f32
        }),
    }
}

#[test]
fn native_provider_read_manual_base_and_reenable_preserve_independent_targets_and_live_history() {
    struct Case {
        definition: &'static dyn NativeDeviceDefinition,
        parameter: ParameterId,
        untouched: ParameterId,
        initial: f32,
        authored: f32,
        manual: f32,
        other: ParameterValue,
    }
    let cases = [
        Case {
            definition: &hex_devices::utility::UTILITY_DEFINITION,
            parameter: hex_devices::utility::parameter::GAIN,
            untouched: hex_devices::utility::parameter::WIDTH,
            initial: 1.0,
            authored: 2.0,
            manual: 0.5,
            other: ParameterValue::Float(1.0),
        },
        Case {
            definition: &hex_devices::saturator::SATURATOR_DEFINITION,
            parameter: hex_devices::saturator::parameter::DRIVE,
            untouched: hex_devices::saturator::parameter::DRY_WET,
            initial: 0.0,
            authored: 18.0,
            manual: -6.0,
            other: ParameterValue::Float(0.5),
        },
        Case {
            definition: &hex_devices::auto_pan::AUTO_PAN_DEFINITION,
            parameter: hex_devices::auto_pan::parameter::AMOUNT,
            untouched: hex_devices::auto_pan::parameter::RATE,
            initial: 0.5,
            authored: 0.75,
            manual: 0.25,
            other: ParameterValue::Float(1.0),
        },
    ];
    for (
        kind,
        &Case {
            definition,
            parameter,
            untouched,
            initial,
            authored,
            manual,
            other,
        },
    ) in cases.iter().enumerate()
    {
        let mut f = fixture(definition, RATE, 512);
        provider_lane(
            &mut f,
            definition,
            parameter,
            ParameterValue::Float(authored),
        );
        provider_lane(&mut f, definition, untouched, other);
        let curves = f
            .editor
            .project()
            .device(f.device)
            .unwrap()
            .automation()
            .to_vec();
        let original_base = f.editor.project().device(f.device).unwrap().state().clone();
        let expected: Vec<_> = (0..BLOCK)
            .map(|i| oracle(kind, i, f64::from(initial), f64::from(authored), true, i))
            .collect();
        for prepared in [false, true] {
            close(&fresh(&f, prepared, BLOCK), &expected);
        }
        let mut live = SessionPlayback::new(revision(&f)).unwrap();
        live.apply(SessionPlaybackCommand::Play).unwrap();
        let mut block = [[0.0; 2]; BLOCK];
        live.render(&mut block).unwrap();
        close(&block, &expected);
        let manual_state = definition
            .patch_parameter(&original_base, parameter, ParameterValue::Float(manual))
            .unwrap();
        f.editor
            .execute(Command::ManualDeviceEdit {
                device: f.device,
                state: manual_state.clone(),
                targets: vec![Target::Parameter(parameter)],
            })
            .unwrap();
        let device = f.editor.project().device(f.device).unwrap();
        assert_eq!(device.state(), &manual_state);
        assert_eq!(
            device.automation_lane(untouched).unwrap().read_selection,
            Read::Authored
        );
        assert_eq!(
            device.automation_lane(parameter).unwrap().read_selection,
            Read::Base
        );
        for lane in device.automation() {
            assert_eq!(
                lane.points,
                curves
                    .iter()
                    .find(|old| old.target == lane.target)
                    .unwrap()
                    .points
            );
        }
        let expected_base: Vec<_> = (0..BLOCK)
            .map(|i| oracle(kind, i, f64::from(manual), f64::from(manual), false, i))
            .collect();
        for prepared in [false, true] {
            close(&fresh(&f, prepared, BLOCK), &expected_base);
        }
        let mut candidate = SessionPlayback::new(revision(&f)).unwrap();
        candidate.try_inherit_state_from(&mut live).unwrap();
        candidate.render(&mut block).unwrap();
        let expected: Vec<_> = (0..BLOCK)
            .map(|i| {
                oracle(
                    kind,
                    BLOCK + i,
                    f64::from(authored),
                    f64::from(manual),
                    true,
                    i,
                )
            })
            .collect();
        close(&block, &expected);
        f.editor
            .execute(Command::SetDeviceAutomationReadSelection {
                device: f.device,
                target: Target::Parameter(parameter),
                selection: Read::Authored,
            })
            .unwrap();
        assert_eq!(
            f.editor.project().device(f.device).unwrap().state(),
            &manual_state
        );
        assert_eq!(
            f.editor.project().device(f.device).unwrap().automation(),
            curves
        );
        let expected: Vec<_> = (0..BLOCK)
            .map(|i| oracle(kind, i, f64::from(manual), f64::from(authored), true, i))
            .collect();
        for prepared in [false, true] {
            close(&fresh(&f, prepared, BLOCK), &expected);
        }
        let mut resumed = SessionPlayback::new(revision(&f)).unwrap();
        resumed.try_inherit_state_from(&mut candidate).unwrap();
        resumed.render(&mut block).unwrap();
        let expected: Vec<_> = (0..BLOCK)
            .map(|i| {
                oracle(
                    kind,
                    2 * BLOCK + i,
                    f64::from(manual),
                    f64::from(authored),
                    true,
                    i,
                )
            })
            .collect();
        close(&block, &expected);
    }
}

fn host_expected(input: HostGain, output: HostGain) -> [f32; 2] {
    SIGNAL.map(|dry| {
        let entering = dry * input.linear();
        let x = f64::from(entering) * 10.0_f64.powf(18.0 / 20.0);
        let wet = if x.abs() <= 0.5 {
            x
        } else {
            x.signum() * (1.0 - 0.25 / x.abs())
        };
        wet as f32 * output.linear()
    })
}
#[test]
fn typed_host_input_and_output_override_independently_around_nonlinear_dsp() {
    let definition = &hex_devices::saturator::SATURATOR_DEFINITION;
    let mut f = fixture(definition, RATE, 512);
    let base = definition
        .patch_parameter(
            f.editor.project().device(f.device).unwrap().state(),
            hex_devices::saturator::parameter::DRIVE,
            ParameterValue::Float(18.0),
        )
        .unwrap();
    f.editor
        .execute(Command::ReplaceDeviceState {
            device: f.device,
            state: base.clone(),
        })
        .unwrap();
    for (target, gain) in [
        (Target::HostInputGain, HostGain::ZERO),
        (
            Target::HostOutputGain,
            HostGain::from_millidb(-6000).unwrap(),
        ),
    ] {
        f.editor
            .execute(Command::ReplaceDeviceAutomationTarget {
                device: f.device,
                target,
                points: vec![point(gain.normalized())],
            })
            .unwrap();
    }
    let original = f
        .editor
        .project()
        .device(f.device)
        .unwrap()
        .automation()
        .to_vec();
    let mut live = SessionPlayback::new(revision(&f)).unwrap();
    live.apply(SessionPlaybackCommand::Play).unwrap();
    let input_base = HostGain::from_millidb(-6000).unwrap();
    let output_curve = HostGain::from_millidb(-6000).unwrap();
    let output_base = HostGain::from_millidb(3000).unwrap();
    let mut block = [[0.0; 2]; BLOCK];
    for stage in 0..5 {
        let (input, output) = match stage {
            0 => (HostGain::ZERO, output_curve),
            1 => {
                f.editor
                    .execute(Command::ManualDeviceEdit {
                        device: f.device,
                        state: base.clone().with_input_gain_millidb(-6000).unwrap(),
                        targets: vec![Target::HostInputGain],
                    })
                    .unwrap();
                assert_eq!(
                    f.editor
                        .project()
                        .device(f.device)
                        .unwrap()
                        .automation_target(Target::HostOutputGain)
                        .unwrap()
                        .read_selection,
                    Read::Authored
                );
                (input_base, output_curve)
            }
            2 => {
                let state = f
                    .editor
                    .project()
                    .device(f.device)
                    .unwrap()
                    .state()
                    .clone()
                    .with_output_gain_millidb(3000)
                    .unwrap();
                f.editor
                    .execute(Command::ManualDeviceEdit {
                        device: f.device,
                        state,
                        targets: vec![Target::HostOutputGain],
                    })
                    .unwrap();
                (input_base, output_base)
            }
            3 => {
                f.editor
                    .execute(Command::SetDeviceAutomationReadSelection {
                        device: f.device,
                        target: Target::HostInputGain,
                        selection: Read::Authored,
                    })
                    .unwrap();
                assert_eq!(
                    f.editor
                        .project()
                        .device(f.device)
                        .unwrap()
                        .automation_target(Target::HostOutputGain)
                        .unwrap()
                        .read_selection,
                    Read::Base
                );
                (HostGain::ZERO, output_base)
            }
            _ => {
                f.editor.execute(Command::ReenableDeviceAutomation).unwrap();
                (HostGain::ZERO, output_curve)
            }
        };
        let expected = vec![host_expected(input, output); BLOCK];
        for prepared in [false, true] {
            close(&fresh(&f, prepared, BLOCK), &expected);
        }
        if stage != 0 {
            let mut candidate = SessionPlayback::new(revision(&f)).unwrap();
            candidate.try_inherit_state_from(&mut live).unwrap();
            live = candidate;
        }
        live.render(&mut block).unwrap();
        close(&block, &expected);
        for lane in f.editor.project().device(f.device).unwrap().automation() {
            assert_eq!(
                lane.points,
                original
                    .iter()
                    .find(|old| old.target == lane.target)
                    .unwrap()
                    .points
            );
        }
    }
    let state = f.editor.project().device(f.device).unwrap().state();
    assert_eq!(state.input_gain_millidb(), -6000);
    assert_eq!(state.output_gain_millidb(), 3000);
    assert_eq!(
        f.editor.project().device(f.device).unwrap().automation(),
        original
    );
}

#[test]
fn overridden_limiter_output_lane_keeps_points_and_reenables_at_delayed_content_origin() {
    const DELAY: usize = 288;
    const FRAMES: usize = 4096;
    let definition = &hex_devices::limiter::DEFINITION;
    let mut f = fixture(definition, 48000, FRAMES);
    let base = definition
        .patch_parameter(
            f.editor.project().device(f.device).unwrap().state(),
            hex_devices::limiter::parameter::LOOKAHEAD,
            ParameterValue::Choice(2),
        )
        .unwrap();
    f.editor
        .execute(Command::ReplaceDeviceState {
            device: f.device,
            state: base.clone(),
        })
        .unwrap();
    let step = BeatTime::from_raw(1 << 28);
    assert_eq!(
        f.editor.project().tempo_map().beat_to_frame(step).unwrap(),
        1500
    );
    let down = HostGain::from_millidb(-6000).unwrap();
    f.editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device: f.device,
            target: Target::HostOutputGain,
            points: vec![
                point(HostGain::ZERO.normalized()),
                AutomationPoint {
                    beat: step,
                    value: AutomationValue::new(down.normalized()).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                },
            ],
        })
        .unwrap();
    let original = f.editor.project().device(f.device).unwrap().automation()[0]
        .points
        .clone();
    let up = HostGain::from_millidb(3000).unwrap();
    for selection in [Read::Authored, Read::Base, Read::Authored] {
        if selection == Read::Base {
            f.editor
                .execute(Command::ManualDeviceEdit {
                    device: f.device,
                    state: base.clone().with_output_gain_millidb(3000).unwrap(),
                    targets: vec![Target::HostOutputGain],
                })
                .unwrap();
        } else {
            f.editor.execute(Command::ReenableDeviceAutomation).unwrap();
        }
        let expected: Vec<_> = (0..FRAMES + DELAY)
            .map(|frame| {
                if frame < DELAY {
                    [0.0; 2]
                } else {
                    let gain = if selection == Read::Base {
                        up.linear()
                    } else if frame - DELAY < 1500 {
                        1.0
                    } else {
                        down.linear()
                    };
                    SIGNAL.map(|sample| sample * gain)
                }
            })
            .collect();
        for prepared in [false, true] {
            close(&fresh(&f, prepared, expected.len()), &expected);
        }
        assert_eq!(
            f.editor.project().device(f.device).unwrap().automation()[0].points,
            original
        );
    }
    assert_eq!(
        f.editor
            .project()
            .device(f.device)
            .unwrap()
            .state()
            .output_gain_millidb(),
        3000
    );
}
