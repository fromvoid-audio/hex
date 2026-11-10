//! Test-only native aux-key runtime; no host routing or product DSP.
use crate::{
    DeviceResources, NativeInstrumentClock, NativeParameterAutomation, NativeProcessorRuntime,
    StereoAuxiliaryBlock, StereoFrame,
};
use hex_lattice_contract::DeviceContractError;
use std::any::Any;

#[derive(Default)]
pub struct AuxiliaryProbe {
    pub frames_seen: usize,
}

impl NativeProcessorRuntime for AuxiliaryProbe {
    fn process_block_with_auxiliary(
        &mut self,
        block: StereoAuxiliaryBlock<'_>,
        _clock: Option<&dyn NativeInstrumentClock>,
        automation: Option<&dyn NativeParameterAutomation>,
        bypassed: bool,
        _resources: &dyn DeviceResources,
    ) -> Result<(), DeviceContractError> {
        if automation.is_some() {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        let (main, key) = block.into_parts();
        self.frames_seen = self.frames_seen.saturating_add(main.len());
        if !bypassed {
            for (main, key) in main.iter_mut().zip(key) {
                main[0] *= key[0];
                main[1] *= key[1];
            }
        }
        Ok(())
    }
    fn process_block(&mut self, _frames: &mut [StereoFrame], _resources: &dyn DeviceResources) {}
    fn reset(&mut self) {
        self.frames_seen = 0;
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.frames_seen = previous.frames_seen;
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
