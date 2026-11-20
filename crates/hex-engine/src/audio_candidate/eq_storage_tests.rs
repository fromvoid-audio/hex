//! Exercise sparse EQ state through ordinary and closed Arrangement playback.
use super::host_output_modulation_tests::compare_native_and_sealed_pcm;
use crate::Audio;
use hex_devices::eq::{EQ_DEFINITION, EqState, Node, NodeId, NodeParameter, OUTPUT_GAIN};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceAutomationTarget, Editor, Project,
};
use std::cell::Cell;

#[test]
fn sparse_eq_closed_candidate_preserves_filter_gain_automation_and_undo() {
    const RATE: u32 = 48_000;
    const FRAMES: usize = 557;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "sparse eq".into(),
            path: "sparse-eq.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture");
    };
    let mut eq = EqState::default();
    let band = NodeId::new(7).unwrap();
    eq.add(Node::new(band, 780.0, 9.0, 0).unwrap()).unwrap();
    let band_gain = band.parameter(NodeParameter::Gain).unwrap();
    let base = eq.encode().unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: base.clone(),
        })
        .unwrap()
    else {
        panic!("sparse EQ fixture");
    };
    let change = editor.project().tempo_map().frame_to_beat(121).unwrap();
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: DeviceAutomationTarget::Parameter(band.parameter(NodeParameter::Q).unwrap()),
            points: [(BeatTime::ZERO, 0.35), (change, 0.63)]
                .map(|(beat, value)| AutomationPoint {
                    beat,
                    value: AutomationValue::new(value).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                })
                .to_vec(),
        })
        .unwrap();

    let dry: [[f32; 2]; FRAMES] = std::array::from_fn(|frame| {
        let signal = (frame as f32 * 0.102).sin() * 0.125;
        [signal, -signal * 0.7]
    });
    let audio = Audio::new(RATE, dry.to_vec()).unwrap();
    let render = |editor: &Editor| {
        let reference = Cell::new(None);
        compare_native_and_sealed_pcm(
            editor.project(),
            asset,
            &audio,
            |pcm: &[[f32; 2]; FRAMES]| {
                if let Some(expected) = reference.get() {
                    assert_eq!(*pcm, expected);
                } else {
                    assert!(pcm.iter().flatten().all(|sample| sample.is_finite()));
                    reference.set(Some(*pcm));
                }
            },
        );
        reference.get().unwrap()
    };
    let original = render(&editor);
    assert_ne!(original, dry);

    let edited = EQ_DEFINITION
        .patch_parameter(&base, band_gain, ParameterValue::Float(-4.0))
        .and_then(|state| {
            EQ_DEFINITION.patch_parameter(&state, OUTPUT_GAIN, ParameterValue::Float(0.5))
        })
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: edited,
        })
        .unwrap();
    assert_eq!(
        EqState::decode(editor.project().device(device).unwrap().state())
            .unwrap()
            .nodes[0]
            .id,
        band
    );
    assert_ne!(render(&editor), original);
    assert!(editor.undo());
    assert_eq!(render(&editor), original);
}
