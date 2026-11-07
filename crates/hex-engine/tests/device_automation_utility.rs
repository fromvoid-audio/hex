//! Audible canonical envelopes must agree between ordinary/offline playback,
//! prepared playback and the unchanged static provider configuration.
use hex_devices::utility::{MAX_UTILITY_GAIN, UTILITY_TYPE_ID, parameter as p};
use hex_engine::{
    Audio, NativeDeviceClass, PreparedRevision, Renderer, SessionPlayback, SessionPlaybackCommand,
    SineTable, compile,
};
use hex_lattice_contract::{NativeDeviceState, ParameterId, ParameterValue};
use hex_project::{
    AutomationInterpolation as Curve, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceId, Editor, Project,
};
use std::{collections::BTreeMap, sync::Arc};

const RATE: u32 = 8_000;
const QUARTER: usize = 4_000;

struct Fixture {
    editor: Editor,
    device: DeviceId,
    media: BTreeMap<hex_project::AssetId, Audio>,
    frames: usize,
}

fn fixture(frames: usize) -> Fixture {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "source".into(),
            path: "original-automation-fixture.wav".into(),
            source_sample_rate: RATE,
            source_frames: frames as u64,
            playback_frames: frames as u64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import");
    };
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(UTILITY_TYPE_ID).unwrap(),
        })
        .unwrap()
    else {
        panic!("utility");
    };
    Fixture {
        editor,
        device,
        media: BTreeMap::from([(
            asset,
            Audio::new(RATE, vec![[0.125, -0.25]; frames]).unwrap(),
        )]),
        frames,
    }
}

fn point(beat: BeatTime, value: f32, outgoing: Curve) -> AutomationPoint {
    AutomationPoint {
        beat,
        value: AutomationValue::new(value).unwrap(),
        outgoing,
    }
}

fn lane(f: &mut Fixture, parameter: ParameterId, points: Vec<AutomationPoint>) {
    f.editor
        .execute(Command::ReplaceDeviceAutomation {
            device: f.device,
            parameter,
            points,
        })
        .unwrap();
}

fn patch(state: &NativeDeviceState, id: ParameterId, value: ParameterValue) -> NativeDeviceState {
    hex_engine::patch_native_device_parameter_for_class(NativeDeviceClass::Effect, state, id, value)
        .unwrap()
}

fn replace_state(f: &mut Fixture, state: NativeDeviceState) {
    f.editor
        .execute(Command::ReplaceDeviceState {
            device: f.device,
            state,
        })
        .unwrap();
}

fn offline(f: &Fixture, start: usize, frames: usize) -> Vec<[f32; 2]> {
    let graph = compile::project(f.editor.project(), &f.media).unwrap();
    let mut renderer = Renderer::new(&graph);
    renderer.seek(start as u64);
    renderer.play();
    let mut output = vec![[0.0; 2]; frames];
    renderer.render(&mut output).unwrap();
    output
}

fn prepared(f: &Fixture, partitions: &[usize]) -> Vec<[f32; 2]> {
    let revision = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = vec![[0.0; 2]; f.frames];
    let mut from = 0;
    for size in partitions.iter().copied().cycle() {
        if from == output.len() {
            break;
        }
        let end = (from + size).min(output.len());
        playback.render(&mut output[from..end]).unwrap();
        from = end;
    }
    output
}

fn close(actual: &[[f32; 2]], expected: &[[f32; 2]], context: &str) {
    assert_eq!(actual.len(), expected.len());
    for (frame, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for channel in 0..2 {
            assert!(
                (actual[channel] - expected[channel]).abs() <= 2.0e-6,
                "{context}: frame {frame}, channel {channel}: {actual:?} != {expected:?}"
            );
        }
    }
}

#[test]
fn linear_gain_and_discrete_channel_and_mute_use_arrangement_time_in_both_renderers() {
    let mut f = fixture(2 * QUARTER + 1);
    let authored = f.editor.project().device(f.device).unwrap().state().clone();
    lane(
        &mut f,
        p::GAIN,
        vec![
            point(BeatTime::ZERO, 0.0, Curve::Linear),
            point(BeatTime::ONE, 2.0 / MAX_UTILITY_GAIN, Curve::Linear),
        ],
    );
    // Discrete providers hold the left value until the next authored beat even
    // when the generic point's outgoing shape is linear.
    lane(
        &mut f,
        p::CHANNEL,
        vec![
            point(BeatTime::ZERO, 0.0, Curve::Linear),
            point(BeatTime::ONE, 1.0, Curve::Linear),
        ],
    );
    lane(
        &mut f,
        p::MUTE,
        vec![
            point(BeatTime::ZERO, 0.0, Curve::Linear),
            point(BeatTime::from_quarters(2).unwrap(), 1.0, Curve::Step),
        ],
    );
    let expected: Vec<_> = (0..f.frames)
        .map(|frame| {
            if frame == 2 * QUARTER {
                [0.0; 2]
            } else {
                let gain = (2.0 * frame as f64 / QUARTER as f64).min(2.0) as f32;
                let input = if frame < QUARTER {
                    [0.125, -0.25]
                } else {
                    [-0.25, 0.125]
                };
                [input[0] * gain, input[1] * gain]
            }
        })
        .collect();
    close(&offline(&f, 0, f.frames), &expected, "offline");
    close(&prepared(&f, &[17, 128, 1, 257]), &expected, "partitions");
    close(
        &offline(&f, QUARTER, 32),
        &expected[QUARTER..QUARTER + 32],
        "seek",
    );
    assert_eq!(
        f.editor.project().device(f.device).unwrap().state(),
        &authored
    );
}

#[test]
fn clearing_restores_authored_base_and_undo_and_snapshot_restore_audible_curve() {
    let mut f = fixture(32);
    let base = patch(
        f.editor.project().device(f.device).unwrap().state(),
        p::GAIN,
        ParameterValue::Float(0.5),
    );
    replace_state(&mut f, base.clone());
    let dry = offline(&f, 0, f.frames);
    lane(
        &mut f,
        p::GAIN,
        vec![point(BeatTime::ZERO, 1.0 / MAX_UTILITY_GAIN, Curve::Step)],
    );
    let active = offline(&f, 0, f.frames);
    close(
        &active,
        &vec![[0.125, -0.25]; f.frames],
        "automation overrides base",
    );
    lane(&mut f, p::GAIN, Vec::new());
    close(&offline(&f, 0, f.frames), &dry, "clear restores base");
    assert!(f.editor.undo());
    close(&offline(&f, 0, f.frames), &active, "undo restores curve");
    f.editor = Editor::new(
        Project::from_snapshot(f.editor.project().snapshot()).unwrap(),
        16,
    )
    .unwrap();
    close(&prepared(&f, &[7, 11]), &active, "snapshot roundtrip");
    assert_eq!(f.editor.project().device(f.device).unwrap().state(), &base);
}

#[test]
fn every_native_utility_parameter_matches_its_static_provider_value() {
    let cases = [
        (p::GAIN, ParameterValue::Float(0.75)),
        (p::BALANCE, ParameterValue::Float(0.3)),
        (p::WIDTH, ParameterValue::Float(2.0)),
        (p::CHANNEL, ParameterValue::Choice(3)),
        (p::INVERT_LEFT, ParameterValue::Bool(true)),
        (p::INVERT_RIGHT, ParameterValue::Bool(true)),
        (p::MONO, ParameterValue::Bool(true)),
        (p::MUTE, ParameterValue::Bool(true)),
        (p::MID_SIDE_ENABLED, ParameterValue::Bool(true)),
        (p::MID_SIDE_BALANCE, ParameterValue::Float(0.4)),
        (p::DC_REMOVE, ParameterValue::Bool(true)),
        (p::BASS_MONO, ParameterValue::Bool(true)),
        (p::BASS_MONO_CUTOFF, ParameterValue::Float(250.0)),
        (p::BASS_MONO_AUDITION, ParameterValue::Bool(true)),
    ];
    for (id, value) in cases {
        let mut f = fixture(256);
        let mut base = f.editor.project().device(f.device).unwrap().state().clone();
        if id == p::MID_SIDE_ENABLED {
            base = patch(&base, p::MID_SIDE_BALANCE, ParameterValue::Float(0.4));
        }
        if id == p::MID_SIDE_BALANCE {
            base = patch(&base, p::MID_SIDE_ENABLED, ParameterValue::Bool(true));
        }
        if id == p::BASS_MONO_CUTOFF || id == p::BASS_MONO_AUDITION {
            base = patch(&base, p::BASS_MONO, ParameterValue::Bool(true));
        }
        replace_state(&mut f, patch(&base, id, value));
        let expected = offline(&f, 0, f.frames);
        replace_state(&mut f, base);
        let metadata = hex_engine::native_device_view_for_class(
            NativeDeviceClass::Effect,
            f.editor.project().device(f.device).unwrap().state(),
        )
        .unwrap();
        let parameter = metadata
            .parameters
            .iter()
            .find(|parameter| parameter.descriptor.id == id)
            .unwrap();
        let normalized = parameter
            .semantics
            .unwrap()
            .normalization
            .normalize(parameter.descriptor, value)
            .unwrap();
        lane(
            &mut f,
            id,
            vec![point(BeatTime::ZERO, normalized, Curve::Step)],
        );
        let label = format!("parameter {}", id.value());
        close(&offline(&f, 0, f.frames), &expected, &label);
        close(&prepared(&f, &[13, 5, 127]), &expected, &label);
    }
}

#[test]
fn transport_loop_uses_arrangement_origin_while_linear_export_keeps_advancing() {
    let mut f = fixture(2 * QUARTER + 1);
    lane(
        &mut f,
        p::GAIN,
        vec![
            point(BeatTime::ZERO, 0.0, Curve::Linear),
            point(BeatTime::ONE, 2.0 / MAX_UTILITY_GAIN, Curve::Step),
        ],
    );
    f.editor
        .execute(Command::SetLoopRegion {
            start: 1_000,
            end: 3_000,
            enabled: true,
        })
        .unwrap();
    let expected: Vec<_> = (0..f.frames)
        .map(|frame| {
            let origin = if frame < 3_000 {
                frame
            } else {
                1_000 + (frame - 3_000) % 2_000
            };
            let gain = 2.0 * origin as f32 / QUARTER as f32;
            // At 8 kHz the end envelope spans 21 frames. Its first
            // sample is unity and its final sample is zero; automation
            // still follows the wrapped Arrangement origin.
            let fade = if origin >= 2_979 {
                (2_999 - origin) as f32 / 20.0
            } else {
                1.0
            };
            [0.125 * gain * fade, -0.25 * gain * fade]
        })
        .collect();
    close(&prepared(&f, &[257, 13, 64]), &expected, "transport loop");
    let linear: Vec<_> = (0..f.frames)
        .map(|frame| {
            let gain = (2.0 * frame as f32 / QUARTER as f32).min(2.0);
            [0.125 * gain, -0.25 * gain]
        })
        .collect();
    close(
        &offline(&f, 0, f.frames),
        &linear,
        "offline ignores transport loop",
    );
}

#[test]
fn main_and_return_curves_keep_their_owner_and_bypass_preserves_dry_audio() {
    for on_main in [true, false] {
        let mut f = fixture(32);
        f.editor.execute(Command::RemoveDevice(f.device)).unwrap();
        let state = hex_engine::default_native_device_state(UTILITY_TYPE_ID).unwrap();
        let command = if on_main {
            Command::InsertMainDevice {
                before: None,
                state,
            }
        } else {
            let Created::ReturnTrack(return_track) = f
                .editor
                .execute(Command::AddReturnTrack {
                    name: "return".into(),
                })
                .unwrap()
            else {
                panic!("return");
            };
            let track = f.editor.project().tracks()[0].id();
            f.editor
                .execute(Command::SetReturnSend {
                    track,
                    return_track,
                    gain: 1.0,
                })
                .unwrap();
            Command::InsertReturnDevice {
                return_track,
                before: None,
                state,
            }
        };
        let Created::Device(device) = f.editor.execute(command).unwrap() else {
            panic!("device");
        };
        f.device = device;
        lane(
            &mut f,
            p::GAIN,
            vec![point(BeatTime::ZERO, 0.0, Curve::Step)],
        );
        let expected = vec![if on_main { [0.0; 2] } else { [0.125, -0.25] }; f.frames];
        close(&offline(&f, 0, f.frames), &expected, "owner offline");
        close(&prepared(&f, &[3, 17]), &expected, "owner prepared");
        f.editor
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
        let expected = vec![
            if on_main {
                [0.125, -0.25]
            } else {
                [0.25, -0.5]
            };
            f.frames
        ];
        close(&offline(&f, 0, f.frames), &expected, "owner bypass");
        close(&prepared(&f, &[7]), &expected, "owner prepared bypass");
    }
}
