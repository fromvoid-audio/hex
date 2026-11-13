//! Sample-rate input trim and its causal follower tap. The tap sees the
//! authored/automated input and the current matrix before any input modulation.
//! Only the actual signal advances the transaction's matrix and host gains.
use crate::matrix_ramp::MatrixRamp;
use crate::{ModulationBlock, ParameterAutomationBlock, StereoFrame, StereoMatrix};
use hex_lattice_contract::{DeviceAutomationTarget, DeviceContractError};

pub(super) const TAP_FRAMES: usize = 128;
const TARGET: DeviceAutomationTarget = DeviceAutomationTarget::HostInputGain;

pub(super) fn prepare_tap(
    tap: &mut [StereoFrame],
    dry: &[StereoFrame],
    base: f32,
    automation: &ParameterAutomationBlock<'_>,
    offset: usize,
    mut matrix_ramp: MatrixRamp,
    matrix: StereoMatrix,
) {
    for (index, (tap, dry)) in tap.iter_mut().zip(dry).enumerate() {
        let gain = automation
            .host_gain_at(TARGET, offset + index)
            .unwrap_or(base);
        *tap = [dry[0] * gain, dry[1] * gain];
    }
    matrix_ramp.apply_block(matrix, tap);
}

pub(super) fn apply(
    automation: &ParameterAutomationBlock<'_>,
    modulation: &ModulationBlock<'_>,
    offset: usize,
    frames: &mut [StereoFrame],
) -> Result<Option<f32>, DeviceContractError> {
    let mut last = None;
    for (index, frame) in frames.iter_mut().enumerate() {
        let upstream = automation.host_gain_normalized_at(TARGET, offset + index);
        let gain = modulation.host_input_gain_at(index, upstream)?.linear();
        frame[0] *= gain;
        frame[1] *= gain;
        last = Some(gain);
    }
    Ok(last)
}
