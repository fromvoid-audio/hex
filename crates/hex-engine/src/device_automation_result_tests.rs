//! A late automation refusal must cross the real render transaction, preserve
//! its cursor, roll back prior speculative DSP, and permit an exact retry.
use crate::{Audio, Renderer, compile};
use hex_lattice::{
    DeviceResources, NativeInstrumentClock, NativeParameterAutomation, NativeProcessorRuntime,
    PreparedNativeProcessor, PreparedProcessorFactory, StereoFrame,
};
use hex_lattice_contract::{
    DeviceContractError as Error, NativeDeviceMetadata, ParameterDescriptor, ParameterId,
    ParameterValue,
};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created, Editor,
    Project,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
struct Factory(Arc<AtomicBool>);
struct Runtime {
    gate: Arc<AtomicBool>,
    sum: f32,
}
fn descriptor() -> ParameterDescriptor {
    hex_devices::utility::UTILITY_DEFINITION.descriptors()[0]
}
impl PreparedNativeProcessor for Factory {
    fn parameter_automation_descriptor(&self, _: ParameterId) -> Option<ParameterDescriptor> {
        Some(descriptor())
    }
    fn supports_parameter_automation(&self, _: ParameterId) -> bool {
        true
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(Runtime {
            gate: self.0.clone(),
            sum: 0.0,
        })
    }
}
impl NativeProcessorRuntime for Runtime {
    fn supports_parameter_automation(&self, _: ParameterId) -> bool {
        true
    }
    fn process_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        values: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) -> Result<(), Error> {
        for (offset, frame) in frames.iter_mut().enumerate() {
            let mut value = [None];
            values.values_at(offset, &mut value)?;
            if offset == 1 && self.gate.load(Ordering::Relaxed) {
                return Err(Error::InvalidParameterValue);
            }
            if let Some(ParameterValue::Float(value)) = value[0] {
                self.sum += value;
            }
            *frame = [self.sum; 2];
        }
        Ok(())
    }
    fn process_block(&mut self, _: &mut [StereoFrame], _: &dyn DeviceResources) {}
    fn reset(&mut self) {
        self.sum = 0.0;
    }
    fn can_copy_history_from(&self, old: &dyn NativeProcessorRuntime) -> bool {
        old.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, old: &dyn NativeProcessorRuntime) {
        if let Some(old) = old.as_any().downcast_ref::<Self>() {
            self.sum = old.sum;
        }
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
#[test]
fn late_automation_refusal_preserves_render_cursor_and_committed_history_on_retry() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = e
        .execute(Command::ImportPreparedAudioTrack {
            name: "original".into(),
            path: "automation-refusal-original.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 8,
            playback_frames: 8,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let Created::Device(device) = e
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_devices::utility::UTILITY_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    e.execute(Command::ReplaceDeviceAutomation {
        device,
        parameter: descriptor().id,
        points: vec![AutomationPoint {
            beat: BeatTime::ZERO,
            value: AutomationValue::new(0.5).unwrap(),
            outgoing: AutomationInterpolation::Step,
        }],
    })
    .unwrap();
    let media = std::collections::BTreeMap::from([(
        asset,
        Audio::new(48_000, vec![[0.125, -0.25]; 8]).unwrap(),
    )]);
    let mut graph = compile::project(e.project(), &media).unwrap();
    let gate = Arc::new(AtomicBool::new(true));
    Arc::make_mut(&mut graph.tracks[0].device_chain)[0].definition = Some(
        PreparedProcessorFactory::Legacy(Arc::new(Factory(gate.clone()))),
    );
    let mut renderer = Renderer::new(&graph);
    renderer.play();
    let mut output = [[1.0; 2]; 8];
    assert!(renderer.render(&mut output).is_err());
    assert_eq!(renderer.position(), 0);
    assert_eq!(output, [[0.0; 2]; 8]);
    gate.store(false, Ordering::Relaxed);
    renderer.render(&mut output).unwrap();
    assert_eq!(renderer.position(), 8);
    let mut pristine = Renderer::new(&graph);
    pristine.play();
    let mut expected = [[0.0; 2]; 8];
    pristine.render(&mut expected).unwrap();
    assert_eq!(output, expected);
}

#[test]
fn recorded_touch_replays_half_open_and_preserves_original_pcm_across_partitions() {
    use hex_lattice_contract::HostGain;
    use hex_project::{
        DeviceAutomationTarget, DeviceAutomationTouchSpan, DeviceAutomationTouchStep,
    };

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "touch playback".into(),
            path: "touch-playback.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 256,
            playback_frames: 256,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("audio import");
    };
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_devices::utility::UTILITY_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("main device");
    };
    let beat = |frame| editor.project().tempo_map().frame_to_beat(frame).unwrap();
    let [start, middle, end, ramp_end] = [16, 23, 31, 64].map(beat);
    let target = DeviceAutomationTarget::HostOutputGain;
    let normalized = |value| AutomationValue::new(value).unwrap();
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target,
            points: vec![
                AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: normalized(0.5),
                    outgoing: AutomationInterpolation::Linear,
                },
                AutomationPoint {
                    beat: ramp_end,
                    value: normalized(0.75),
                    outgoing: AutomationInterpolation::Step,
                },
            ],
        })
        .unwrap();
    let media = std::collections::BTreeMap::from([(
        asset,
        Audio::new(48_000, vec![[0.125, -0.25]; 256]).unwrap(),
    )]);
    let render = |project: &Project, partition| {
        let graph = compile::project(project, &media).unwrap();
        let mut renderer = Renderer::new(&graph);
        renderer.play();
        let mut pcm = [[0.0; 2]; 64];
        for segment in pcm.chunks_mut(partition) {
            renderer.render(segment).unwrap();
        }
        pcm
    };
    let base = render(editor.project(), 64);
    editor
        .execute(Command::SpliceDeviceAutomationTouch {
            device,
            target,
            spans: vec![DeviceAutomationTouchSpan {
                start,
                end,
                steps: vec![
                    DeviceAutomationTouchStep {
                        beat: start,
                        value: normalized(0.25),
                    },
                    DeviceAutomationTouchStep {
                        beat: middle,
                        value: normalized(0.875),
                    },
                ],
            }],
        })
        .unwrap();
    let actual = render(editor.project(), 7);
    assert_eq!(actual, render(editor.project(), 64));
    assert_eq!(actual[..16], base[..16]);
    assert_eq!(actual[31..], base[31..]);
    for (range, normalized) in [(16..23, 0.25), (23..31, 0.875)] {
        let gain = HostGain::from_normalized(normalized).unwrap().linear();
        for frame in &actual[range] {
            assert_eq!(*frame, [0.125 * gain, -0.25 * gain]);
        }
    }
}
