//! Closed Limiter custody preserves the registry processor and its transactions.
use super::host_output_modulation_tests::compare_native_and_sealed_pcm;
use crate::Audio;
use hex_devices::limiter::{DEFINITION, parameter};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceAutomationTarget, Editor, Project,
};
use std::cell::RefCell;

#[test]
fn closed_limiter_matches_registry_for_fixed_and_automated_layouts() {
    const RATE: u32 = 8_000;
    const FRAMES: usize = 1024;
    let signal: Vec<_> = (0..FRAMES)
        .map(|i| {
            let v = ((i % 73) as f32 - 36.0) / 32.0;
            [v, -v * 0.7]
        })
        .collect();
    let audio = Audio::new(RATE, signal).unwrap();
    for target in [
        None,
        Some(parameter::GAIN),
        Some(parameter::LOOKAHEAD),
        Some(parameter::ROUTING),
        Some(parameter::MODE),
    ] {
        for bypassed in [false, true] {
            for main in [false, true] {
                let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
                let Created::Imported { track, asset, .. } = editor
                    .execute(Command::ImportAudioTrack {
                        name: "limiter".into(),
                        path: "limiter.wav".into(),
                        sample_rate: RATE,
                        frames: FRAMES as u64,
                    })
                    .unwrap()
                else {
                    panic!("audio fixture")
                };
                let state = DEFINITION.default_state().unwrap();
                let command = if main {
                    Command::InsertMainDevice {
                        before: None,
                        state,
                    }
                } else {
                    Command::InsertDevice {
                        track,
                        before: None,
                        state,
                    }
                };
                let Created::Device(device) = editor.execute(command).unwrap() else {
                    panic!("limiter fixture")
                };
                if let Some(id) = target {
                    let change = editor.project().tempo_map().frame_to_beat(256).unwrap();
                    editor
                        .execute(Command::ReplaceDeviceAutomationTarget {
                            device,
                            target: DeviceAutomationTarget::Parameter(id),
                            points: [(BeatTime::ZERO, 0.0), (change, 1.0)]
                                .map(|(beat, value)| AutomationPoint {
                                    beat,
                                    value: AutomationValue::new(value).unwrap(),
                                    outgoing: AutomationInterpolation::Step,
                                })
                                .to_vec(),
                        })
                        .unwrap();
                }
                editor
                    .execute(Command::SetDeviceBypass { device, bypassed })
                    .unwrap();
                let expected = RefCell::new(None);
                compare_native_and_sealed_pcm::<FRAMES>(editor.project(), asset, &audio, |pcm| {
                    let reference = *expected.borrow();
                    if let Some(reference) = reference {
                        assert_eq!(
                            *pcm, reference,
                            "target {target:?}, bypassed {bypassed}, main {main}"
                        );
                    } else {
                        assert!(pcm.iter().flatten().any(|v| v.abs() > 0.01));
                        *expected.borrow_mut() = Some(*pcm);
                    }
                });
            }
        }
    }
}
