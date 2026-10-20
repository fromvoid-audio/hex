//! An admitted zero-latency routed keyboard monitor still has a signal clock.
use crate::signal_graph::tests::{Definition, Fixture, fixture, state};
use crate::{Frame, PreparedRevision};
use hex_lattice::{
    NativeDeviceDefinition, NativeDeviceRegistry, NativeInstrumentClock, NativeProcessorRuntime,
    OutputTail, PreparedNativeProcessor, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use hex_project::{Command, Created};
use std::{any::Any, sync::Arc};

struct ClockDefinition;
impl NativeDeviceMetadata for ClockDefinition {
    fn type_id(&self) -> DeviceTypeId {
        NativeDeviceMetadata::type_id(&Definition)
    }
    fn state_version(&self) -> u32 {
        Definition.state_version()
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        Definition.descriptors()
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Definition.default_state()
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Definition.migrate_state(state)
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Definition.read_parameter(state, parameter)
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Definition.patch_parameter(state, parameter, value)
    }
}
impl NativeDeviceDefinition for ClockDefinition {
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Definition.prepare_matrix(state)
    }
    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        _: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        self.migrate_state(state)?;
        Ok(Some(Box::new(ClockPrepared)))
    }
}
struct ClockPrepared;
impl PreparedNativeProcessor for ClockPrepared {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn declared_runtime_bytes(&self) -> usize {
        size_of::<ClockRuntime>()
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 0 }
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(ClockRuntime)
    }
}
struct ClockRuntime;
impl NativeProcessorRuntime for ClockRuntime {
    fn process_block(&mut self, frames: &mut [Frame], _: &dyn hex_lattice::DeviceResources) {
        frames.fill([0.0; 2]);
    }
    fn process_block_with_clock(
        &mut self,
        frames: &mut [Frame],
        clock: Option<&dyn NativeInstrumentClock>,
        _: &dyn hex_lattice::DeviceResources,
    ) {
        for (offset, frame) in frames.iter_mut().enumerate() {
            let valid = clock.is_some_and(|clock| {
                clock.beat_at_offset(offset) == Some(1 << 32)
                    && clock.arrangement_beat_at_offset(offset) == Some(1 << 32)
                    && clock.arrangement_frame_at_offset(offset) == Some(24_000)
                    && clock.performance_frame_at_offset(offset).is_none()
                    && clock.loop_pass_at_offset(offset) == Some(0)
                    && clock.transport_is_running() == Some(false)
            });
            if !valid {
                *frame = [0.0; 2];
            }
        }
    }
    fn reset(&mut self) {}
    fn is_quiescent(&self) -> bool {
        true
    }
    fn can_copy_history_from(&self, old: &dyn NativeProcessorRuntime) -> bool {
        old.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, _: &dyn NativeProcessorRuntime) {}
    fn as_any(&self) -> &dyn Any {
        self
    }
}
fn monitor(f: &Fixture) -> Vec<Frame> {
    let definition = ClockDefinition;
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let effects = NativeDeviceRegistry::new(&definitions).unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_registries(
            f.editor.project(),
            &f.media,
            crate::SharedSineTable::new_on_control(),
            None,
            &effects,
            &instruments,
        )
        .unwrap(),
    );
    let graph = revision.arrangement().signal_graph.as_ref().unwrap();
    assert!(
        graph.needs_runtime(),
        "actual typed audio route selects compensation"
    );
    assert_eq!(
        graph.plan.max_latency_frames(),
        0,
        "no positive-latency monitoring expansion"
    );
    let mut playback = crate::playback::Playback::try_from_prepared_with_session(revision).unwrap();
    playback.apply(crate::playback::PlaybackCommand::Seek(24_000));
    let keys = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
    playback
        .prepare_computer_keyboard(f.editor.project(), f.tracks[0], Arc::clone(&keys))
        .unwrap();
    keys.set_key(0, Some((69, 100, 1))).unwrap();
    let mut output = vec![[0.0; 2]; 256];
    playback.render(&mut output).unwrap();
    assert!(
        !playback.playing(),
        "keyboard monitoring must keep transport stopped"
    );
    output
}
#[test]
fn admitted_routed_keyboard_main_receives_each_frozen_complete_origin_clock() {
    let mut f = fixture(&[&[], &[]], 64);
    f.editor
        .execute(Command::InsertInstrument {
            track: f.tracks[0],
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    f.editor
        .execute(Command::SetAudioFeed {
            source: hex_project::RoutingOwner::Track(f.tracks[0]),
            destination: f.tracks[1],
            gain: 1.0,
        })
        .unwrap();
    let Created::Device(main) = f
        .editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(0).unwrap(),
        })
        .unwrap()
    else {
        panic!("main")
    };
    f.editor
        .execute(Command::SetDeviceBypass {
            device: main,
            bypassed: true,
        })
        .unwrap();
    let expected = monitor(&f);
    assert!(
        expected.iter().flatten().any(|sample| *sample != 0.0),
        "real held keyboard input must be audible"
    );
    f.editor
        .execute(Command::SetDeviceBypass {
            device: main,
            bypassed: false,
        })
        .unwrap();
    assert_eq!(
        monitor(&f),
        expected,
        "clock-driven identity must preserve admitted stopped-monitor PCM"
    );
}
