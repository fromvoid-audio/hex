//! Validate the exact prepared and paired runtime custody of closed EQ8.
use super::host_output_modulation_tests::compare_native_and_sealed_pcm;
use crate::Audio;
use hex_devices::eq8::{EQ8_DEFINITION, parameter};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceAutomationTarget, Editor, Project,
};
use std::cell::Cell;

#[test]
fn sealed_eq8_matches_ordinary_pcm_during_a_mode_fade_and_automated_gain() {
    const RATE: u32 = 48_000;
    const FRAMES: usize = 557;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "eq eight".into(),
            path: "eq-eight.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture");
    };
    let mut state = EQ8_DEFINITION.default_state().unwrap();
    for (id, value) in [
        (parameter::enabled(0, 0), ParameterValue::Bool(true)),
        (parameter::gain(0, 0), ParameterValue::Float(8.0)),
        (parameter::enabled(1, 0), ParameterValue::Bool(true)),
        (parameter::gain(1, 0), ParameterValue::Float(-12.0)),
    ] {
        state = EQ8_DEFINITION.patch_parameter(&state, id, value).unwrap();
    }
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("eq8 fixture");
    };
    let change = editor.project().tempo_map().frame_to_beat(101).unwrap();
    for (parameter, start, end) in [
        (parameter::MODE, 0.0, 0.5),
        (parameter::OUTPUT_GAIN, 0.25, 0.75),
    ] {
        editor
            .execute(Command::ReplaceDeviceAutomationTarget {
                device,
                target: DeviceAutomationTarget::Parameter(parameter),
                points: [(BeatTime::ZERO, start), (change, end)]
                    .map(|(beat, value)| AutomationPoint {
                        beat,
                        value: AutomationValue::new(value).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    })
                    .to_vec(),
            })
            .unwrap();
    }

    let dry: [[f32; 2]; FRAMES] = std::array::from_fn(|frame| {
        let signal = (frame as f32 * 0.08).sin() * 0.3;
        [signal, -signal * 0.7]
    });
    let audio = Audio::new(RATE, dry.to_vec()).unwrap();
    let reference = Cell::new(None);
    compare_native_and_sealed_pcm::<FRAMES>(editor.project(), asset, &audio, |pcm| {
        if let Some(expected) = reference.get() {
            assert_eq!(*pcm, expected);
        } else {
            assert!(pcm.iter().flatten().all(|sample| sample.is_finite()));
            assert_ne!(*pcm, dry);
            reference.set(Some(*pcm));
        }
    });
}
