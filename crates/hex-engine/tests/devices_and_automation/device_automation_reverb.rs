//! Reverb's shared lane controls the audible blend while its wet tank retains history.
use hex_devices::reverb::{REVERB_DEFINITION, REVERB_TYPE_ID, parameter as p};
use hex_engine::{
    Audio, PreparedRevision, Renderer, SessionPlayback, SessionPlaybackCommand, SineTable, compile,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{
    AutomationInterpolation as Shape, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    Editor, Project,
};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn reverb_dry_wet_lane_uses_sample_aligned_step_and_linear_blends() {
    const RATE: u32 = 8000;
    const FRAMES: usize = 8192;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "room automation".into(),
            path: "room-automation.wav".into(),
            source_sample_rate: RATE,
            source_frames: FRAMES as u64,
            playback_frames: FRAMES as u64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("expected imported audio");
    };
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(REVERB_TYPE_ID).unwrap(),
        })
        .unwrap()
    else {
        panic!("expected reverb device");
    };
    let base = editor.project().device(device).unwrap().state().clone();
    let dry = [0.125, -0.0625];
    let media = BTreeMap::from([(asset, Audio::new(RATE, vec![dry; FRAMES]).unwrap())]);
    let offline = |project: &Project| {
        let graph = compile::project(project, &media).unwrap();
        let mut renderer = Renderer::new(&graph);
        renderer.play();
        let mut samples = vec![[0.0; 2]; FRAMES];
        renderer.render(&mut samples).unwrap();
        samples
    };
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: REVERB_DEFINITION
                .patch_parameter(&base, p::DRY_WET, ParameterValue::Float(100.0))
                .unwrap(),
        })
        .unwrap();
    let wet = offline(editor.project());
    assert!(wet.iter().any(|sample| sample[0].abs() > 0.01));
    assert!(editor.undo());
    editor
        .execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: p::DRY_WET,
            points: [
                (0, 0.0, Shape::Step),
                (1, 0.5, Shape::Linear),
                (2, 1.0, Shape::Step),
            ]
            .map(|(beat, value, outgoing)| AutomationPoint {
                beat: BeatTime::from_quarters(beat).unwrap(),
                value: AutomationValue::new(value).unwrap(),
                outgoing,
            })
            .to_vec(),
        })
        .unwrap();
    let expected = offline(editor.project());
    for (index, (mixed, room)) in expected.iter().zip(&wet).enumerate() {
        let mix = if index < 4000 {
            0.0
        } else {
            0.5 + 0.5 * ((index - 4000) as f32 / 4000.0).min(1.0)
        };
        for channel in 0..2 {
            let target = (1.0 - mix) * dry[channel] + mix * room[channel];
            assert!(
                (mixed[channel] - target).abs() < 2e-6,
                "frame {index}: {mixed:?}"
            );
        }
    }
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    let mut prepared = SessionPlayback::new(revision).unwrap();
    prepared.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual = vec![[0.0; 2]; FRAMES];
    let mut offset = 0;
    for width in [13, 257, 3, 64].into_iter().cycle() {
        if offset == FRAMES {
            break;
        }
        let end = (offset + width).min(FRAMES);
        prepared.render(&mut actual[offset..end]).unwrap();
        offset = end;
    }
    assert_eq!(actual, expected);
    assert_eq!(editor.project().device(device).unwrap().state(), &base);
}
