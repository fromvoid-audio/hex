//! Independent early/diffuse decomposition proves real output-control lanes.
use hex_devices::reverb::{REVERB_DEFINITION as D, parameter as p};
use hex_engine::{
    Audio, PreparedRevision, Renderer, SessionPlayback, SessionPlaybackCommand, SineTable, compile,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
use hex_project::{
    AutomationInterpolation as Shape, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    Editor, Project,
};
use std::{collections::BTreeMap, sync::Arc};
fn point(
    id: ParameterId,
    beat: u64,
    value: f32,
    outgoing: Shape,
    state: &hex_lattice_contract::NativeDeviceState,
) -> AutomationPoint {
    let descriptor = D.descriptors().iter().find(|d| d.id == id).unwrap();
    let semantics = D.parameter_semantics_for_state(state, id).unwrap().unwrap();
    AutomationPoint {
        beat: BeatTime::from_quarters(beat).unwrap(),
        value: AutomationValue::new(
            semantics
                .normalization
                .normalize(*descriptor, ParameterValue::Float(value))
                .unwrap(),
        )
        .unwrap(),
        outgoing,
    }
}
#[test]
fn reverb_outputs_match_independent_gain_and_width_oracles_on_all_effect_owners() {
    const RATE: u32 = 8000;
    const FRAMES: usize = 8192;
    let dry = [0.125, -0.0625];
    for owner in 0..3 {
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "room outputs".into(),
                path: "original-room-outputs.wav".into(),
                source_sample_rate: RATE,
                source_frames: FRAMES as u64,
                playback_frames: FRAMES as u64,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("track");
        };
        let mut state = D.default_state().unwrap();
        for id in [p::LO_CUT, p::HI_CUT, p::HIGH, p::LOW] {
            state = D
                .patch_parameter(&state, id, ParameterValue::Bool(true))
                .unwrap();
        }
        for (id, value) in [
            (p::DRY_WET, 100.0),
            (p::STEREO, 100.0),
            (p::REFLECT, 0.0),
            (p::DIFFUSE, 0.0),
        ] {
            state = D
                .patch_parameter(&state, id, ParameterValue::Float(value))
                .unwrap();
        }
        let command = match owner {
            0 => Command::InsertDevice {
                track,
                before: None,
                state,
            },
            1 => Command::InsertMainDevice {
                before: None,
                state,
            },
            _ => {
                let Created::ReturnTrack(return_track) = editor
                    .execute(Command::AddReturnTrack {
                        name: "room return".into(),
                    })
                    .unwrap()
                else {
                    panic!("return");
                };
                editor
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
            }
        };
        let Created::Device(device) = editor.execute(command).unwrap() else {
            panic!("device");
        };
        let base = editor.project().device(device).unwrap().state().clone();
        let media = BTreeMap::from([(asset, Audio::new(RATE, vec![dry; FRAMES]).unwrap())]);
        let render = |project: &Project| {
            let graph = compile::project(project, &media).unwrap();
            let mut renderer = Renderer::new(&graph);
            renderer.play();
            let mut out = vec![[0.0; 2]; FRAMES];
            renderer.render(&mut out).unwrap();
            out
        };
        // Held authored controls must reproduce a separately prepared room,
        // including delay positions and oscillator phase at each effect owner.
        let settings = [
            (p::LO_FREQ, 250.0),
            (p::HI_FREQ, 3000.0),
            (p::HIGH_FREQ, 2000.0),
            (p::LOW_FREQ, 500.0),
            (p::LO_Q, 2.0),
            (p::HI_Q, 1.0),
            (p::PREDELAY, 50.0),
            (p::SPIN_AMOUNT, 80.0),
            (p::SPIN_RATE, 2.0),
            (p::SHAPE, 0.1),
            (p::SCALE, 90.0),
            (p::CHORUS_AMOUNT, 3.0),
            (p::CHORUS_RATE, 5.0),
        ];
        let mut authored = Editor::new(editor.project().clone(), 32).unwrap();
        let mut changed = base.clone();
        for (id, value) in settings {
            changed = D
                .patch_parameter(&changed, id, ParameterValue::Float(value))
                .unwrap();
            authored
                .execute(Command::ReplaceDeviceAutomation {
                    device,
                    parameter: id,
                    points: vec![point(id, 0, value, Shape::Step, &base)],
                })
                .unwrap();
        }
        let mut canonical = Editor::new(editor.project().clone(), 4).unwrap();
        canonical
            .execute(Command::ReplaceDeviceState {
                device,
                state: changed,
            })
            .unwrap();
        let reference = render(canonical.project());
        let automated = render(authored.project());
        assert!(
            reference
                .iter()
                .zip(&render(editor.project()))
                .any(|(a, b)| (a[0] - b[0]).abs() > 1e-5)
        );
        for (a, b) in automated.iter().zip(&reference) {
            assert!(
                (a[0] - b[0]).abs() < 2e-6 && (a[1] - b[1]).abs() < 2e-6,
                "owner {owner}: {a:?} != {b:?}"
            );
        }
        let revision = Arc::new(
            PreparedRevision::prepare(authored.project(), &media, Arc::new(SineTable::new()), None)
                .unwrap(),
        );
        let mut playback = SessionPlayback::new(revision).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut chunked = vec![[0.0; 2]; FRAMES];
        for block in chunked.chunks_mut(257) {
            playback.render(block).unwrap();
        }
        assert_eq!(chunked, automated);
        playback.apply(SessionPlaybackCommand::Seek(0)).unwrap();
        playback.render(&mut chunked).unwrap();
        assert_eq!(chunked, automated);
        let unity = render(editor.project());
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: D
                    .patch_parameter(&base, p::REFLECT, ParameterValue::Float(6.0))
                    .unwrap(),
            })
            .unwrap();
        let raised = render(editor.project());
        assert!(editor.undo());
        for (id, a, b) in [
            (p::STEREO, 0.0, 100.0),
            (p::REFLECT, -12.0, 0.0),
            (p::DIFFUSE, 0.0, -12.0),
        ] {
            editor
                .execute(Command::ReplaceDeviceAutomation {
                    device,
                    parameter: id,
                    points: vec![
                        point(id, 0, a, Shape::Linear, &base),
                        point(id, 2, b, Shape::Step, &base),
                    ],
                })
                .unwrap();
        }
        editor
            .execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: p::DRY_WET,
                points: vec![
                    point(p::DRY_WET, 0, 25.0, Shape::Step, &base),
                    point(p::DRY_WET, 1, 75.0, Shape::Linear, &base),
                    point(p::DRY_WET, 2, 50.0, Shape::Step, &base),
                ],
            })
            .unwrap();
        let expected = render(editor.project());
        let reference_gain = 10.0_f32.powf(6.0 / 20.0);
        for (index, sample) in expected.iter().enumerate() {
            let t = (index as f32 / 8000.0).min(1.0);
            let r = 10.0_f32.powf((-12.0 + 12.0 * t) / 20.0);
            let d = 10.0_f32.powf(-12.0 * t / 20.0);
            let room: [f32; 2] = std::array::from_fn(|ch| {
                let original = if owner == 2 { dry[ch] } else { 0.0 };
                let a = unity[index][ch] - original;
                let b = raised[index][ch] - original;
                let early = (b - a) / (reference_gain - 1.0);
                early * r + (a - early) * d
            });
            let mid = (room[0] + room[1]) * 0.5;
            let side = (room[0] - room[1]) * 0.5 * t;
            let mix = if index < 4000 {
                0.25
            } else {
                0.75 - 0.25 * ((index - 4000) as f32 / 4000.0).min(1.0)
            };
            for (ch, wet) in [mid + side, mid - side].into_iter().enumerate() {
                let target =
                    dry[ch] * (1.0 - mix) + wet * mix + if owner == 2 { dry[ch] } else { 0.0 };
                assert!(
                    (sample[ch] - target).abs() < 3e-6,
                    "owner {owner} frame {index}: {sample:?} != {target}"
                );
            }
        }
        let revision = Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
                .unwrap(),
        );
        let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut actual = vec![[0.0; 2]; FRAMES];
        for block in actual.chunks_mut(257) {
            playback.render(block).unwrap();
        }
        assert_eq!(actual, expected);
        playback.apply(SessionPlaybackCommand::Seek(0)).unwrap();
        playback.render(&mut actual).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(editor.project().device(device).unwrap().state(), &base);
    }
}
