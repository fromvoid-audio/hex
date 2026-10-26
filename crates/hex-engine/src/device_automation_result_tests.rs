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
