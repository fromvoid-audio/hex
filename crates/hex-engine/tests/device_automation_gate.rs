//! Gate floor envelopes use canonical timing and retain their release ramp across blocks.
use hex_devices::gate::{GATE_TYPE_ID, parameter as p};
use hex_engine::{
    Audio, PreparedRevision, Renderer, SessionPlayback, SessionPlaybackCommand, SineTable, compile,
};
use hex_project::{
    AutomationInterpolation as Shape, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    Editor, Project,
};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn gate_floor_lane_uses_sample_aligned_step_and_linear_controls() {
    const RATE: u32 = 8000;
    const FRAMES: usize = 8192;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "gate automation".into(),
            path: "gate-automation.wav".into(),
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
            state: hex_engine::default_native_device_state(GATE_TYPE_ID).unwrap(),
        })
        .unwrap()
    else {
        panic!("expected gate device");
    };
    let base = editor.project().device(device).unwrap().state().clone();
    let dry = [0.0001, -0.00005];
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
        .execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: p::FLOOR,
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
    let mut gain = 0.001_f64; // saved -60 db floor
    let mut target = gain;
    let mut step = 0.0;
    let mut remaining = 0;
    for (index, sample) in expected.iter().enumerate() {
        let normalized = if index < 4000 {
            0.0
        } else {
            0.5 + 0.5 * ((index - 4000) as f32 / 4000.0).min(1.0)
        };
        let floor = 10.0_f64.powf(f64::from(-80.0 + 80.0 * normalized) / 20.0);
        if floor != target {
            target = floor;
            remaining = 800; // saved 100 ms release at 8 khz
            step = (target - gain) / 800.0;
        }
        if remaining > 0 {
            remaining -= 1;
            gain = if remaining == 0 { target } else { gain + step };
        }
        for channel in 0..2 {
            assert!(
                (f64::from(sample[channel]) - f64::from(dry[channel]) * gain).abs() < 2e-10,
                "frame {index}: {sample:?}"
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
    prepared.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    prepared.render(&mut actual).unwrap();
    assert_eq!(
        actual, expected,
        "seek resets the gate and binding cache together"
    );
    editor
        .execute(Command::SetLoopRegion {
            start: 4000,
            end: 8000,
            enabled: true,
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    let render_loop = |width: usize| {
        let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut samples = vec![[0.0; 2]; 16013];
        for block in samples.chunks_mut(width) {
            playback.render(block).unwrap();
        }
        samples
    };
    assert_eq!(
        render_loop(16013),
        render_loop(257),
        "loop wraps retain exactly the same envelope across physical cuts"
    );
    assert_eq!(editor.project().device(device).unwrap().state(), &base);
}
