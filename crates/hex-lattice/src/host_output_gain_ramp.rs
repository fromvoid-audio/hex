//! Canonical static output-trim changes across a compatible native effect
//! handoff. The host seeds a 5 ms linear-amplitude ramp from the donor's last
//! committed effective gain, including reversals. Unchanged revisions continue
//! that trajectory, including the wet side of an admitted bypass ramp. New
//! ramps do not start while bypassed; reset, retained tails, foreign processing and an
//! authored/live output-gain program keep their existing laws.
//!
//! Armed transient Set holds its target; explicit Clear blends back to the
//! sample's authored/external fallback. No command continues the admitted
//! trajectory, including when a new command is deferred for lack of origin.
//! Lifecycle invalidation drops the transient explicitly. The receipt marks
//! delivery of the first sample of the change, independent of settling.
//! Copy state lives in the existing committed/scratch transaction and is charged
//! by RuntimeEntry storage accounting. Settled blocks use one constant multiply.
use hex_dsp::StereoFrame;
use hex_lattice_contract::{DeviceAutomationTarget, DeviceContractError, HostGain};

use crate::{
    ModulationBlock, NativeTransientOperation, NativeTransientValue, NormalizedAutomationValue,
    ParameterAutomationBlock, RuntimeEntry,
};

/// Apply an already validated command to the current output stage. Repeated
/// delivery of the same request must not restart a partially rendered ramp.
pub(super) fn stage_transient(
    entry: &mut RuntimeEntry,
    operation: NativeTransientOperation,
) -> Result<(), DeviceContractError> {
    let next = match operation {
        NativeTransientOperation::Set(NativeTransientValue::HostOutputGain(gain)) => {
            TransientGain::Held(gain)
        }
        NativeTransientOperation::Clear(DeviceAutomationTarget::HostOutputGain)
            if matches!(entry.scratch_transient_output_gain, TransientGain::Held(_)) =>
        {
            TransientGain::Releasing
        }
        _ => return Ok(()),
    };
    if matches!(
        entry.runtime,
        crate::runtime_ownership::RuntimeOwnership::Foreign(_)
    ) {
        return Err(DeviceContractError::UnsupportedProcessingClass);
    }
    if next != entry.scratch_transient_output_gain {
        let from = entry.scratch_host_gains[1];
        entry.scratch_output_gain_ramp = match next {
            TransientGain::Held(target) if target.linear() == from => OutputGainRamp::INACTIVE,
            _ => OutputGainRamp::start(from, entry.sample_rate),
        };
        entry.scratch_transient_output_gain = next;
    }
    Ok(())
}

pub(super) fn finish_release(entry: &mut RuntimeEntry) {
    if entry.scratch_transient_output_gain == TransientGain::Releasing
        && !entry.scratch_output_gain_ramp.active()
    {
        entry.scratch_transient_output_gain = TransientGain::Follow;
    }
}

pub(super) fn invalidate_transient(entry: &mut RuntimeEntry) {
    if entry.committed_transient_output_gain != TransientGain::Follow {
        entry.committed_transient_output_gain = TransientGain::Follow;
        entry.scratch_transient_output_gain = TransientGain::Follow;
        entry.committed_output_gain_ramp = OutputGainRamp::INACTIVE;
        entry.scratch_output_gain_ramp = OutputGainRamp::INACTIVE;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(super) enum TransientGain {
    #[default]
    Follow,
    Held(HostGain),
    Releasing,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct OutputGainRamp {
    from: f32,
    elapsed: u32,
    total: u32,
}
impl OutputGainRamp {
    pub(super) const INACTIVE: Self = Self {
        from: 1.0,
        elapsed: 0,
        total: 0,
    };
    pub(super) fn active(self) -> bool {
        self.elapsed < self.total
    }
    fn at(self, target: f32, elapsed: u32) -> f32 {
        if elapsed >= self.total {
            return target;
        }
        (f64::from(self.from)
            + (f64::from(target) - f64::from(self.from)) * f64::from(elapsed)
                / f64::from(self.total)) as f32
    }
    pub(super) fn inherited(
        old: Self,
        old_target: f32,
        effective: f32,
        target: f32,
        rate: u32,
        eligible: bool,
    ) -> Self {
        if !eligible
            || rate == 0
            || !effective.is_finite()
            || effective < 0.0
            || !target.is_finite()
            || target < 0.0
        {
            return Self::INACTIVE;
        }
        if old_target == target {
            return old;
        }
        if effective == target {
            return Self::INACTIVE;
        }
        Self::start(effective, rate)
    }
    pub(super) fn start(from: f32, rate: u32) -> Self {
        Self {
            from,
            elapsed: 0,
            total: rate.div_ceil(200).max(1),
        }
    }
    pub(super) fn next(&mut self, target: f32) -> f32 {
        if self.active() {
            self.elapsed += 1;
        }
        self.at(target, self.elapsed)
    }
    pub(super) fn apply(&mut self, target: f32, frames: &mut [StereoFrame]) -> Option<f32> {
        let mut last = None;
        let mut ramped = 0;
        if self.active() {
            for frame in frames.iter_mut() {
                if !self.active() {
                    break;
                }
                let gain = self.next(target);
                frame[0] *= gain;
                frame[1] *= gain;
                last = Some(gain);
                ramped += 1;
            }
        }
        if let Some(rest) = frames.get_mut(ramped..)
            && !rest.is_empty()
        {
            if target != 1.0 {
                for frame in rest {
                    frame[0] *= target;
                    frame[1] *= target;
                }
            }
            last = Some(target);
        }
        last
    }
}
/// Apply modulated output after the provider using the same scratch gain
/// ramp. The upstream block is indexed at its output origin.
pub(super) fn apply_modulated(
    ramp: &mut OutputGainRamp,
    transient: TransientGain,
    automation: &ParameterAutomationBlock<'_>,
    values: &ModulationBlock<'_>,
    offset: usize,
    frames: &mut [StereoFrame],
) -> Result<Option<f32>, DeviceContractError> {
    let held = match transient {
        TransientGain::Held(gain) => Some(NormalizedAutomationValue::new(gain.normalized())?),
        _ => None,
    };
    let mut last = None;
    for (frame_offset, frame) in frames.iter_mut().enumerate() {
        let upstream = held.or_else(|| {
            automation.host_gain_normalized_at(
                DeviceAutomationTarget::HostOutputGain,
                offset + frame_offset,
            )
        });
        let gain = values
            .host_output_gain_at(frame_offset, upstream)?
            .ok_or(DeviceContractError::InvalidDescriptor)?;
        let gain = ramp.next(gain.linear());
        frame[0] *= gain;
        frame[1] *= gain;
        last = Some(gain);
    }
    Ok(last)
}

/// Carry a canonical static output stage using the existing host-stage donor
/// lookup. Ordinary DSP/source inheritance still owns eligibility; this adds no
/// search pass, source fact, or transient Set/Clear interpretation.
pub(super) fn inherit(
    entry: &mut crate::RuntimeEntry,
    old: Option<(usize, &crate::RuntimeEntry)>,
    excluded: &std::ops::Range<usize>,
) {
    use crate::runtime_ownership::RuntimeOwnership;
    use hex_lattice_contract::DeviceAutomationTarget::HostOutputGain;
    let mut ramp = OutputGainRamp::INACTIVE;
    if let Some((index, old)) = old {
        // Most graph publications do not change this stage. Do not
        // recheck payloads or seed new control state for inactive gains.
        if entry.output_gain == old.output_gain && !old.committed_output_gain_ramp.active() {
            entry.committed_output_gain_ramp = OutputGainRamp::INACTIVE;
            entry.scratch_output_gain_ramp = OutputGainRamp::INACTIVE;
            return;
        }
        let configuration_present = entry.configuration.is_some() && old.configuration.is_some();
        // An unchanged host target continues independently of a
        // simultaneously retargeted device payload/input/matrix. Its
        // existing trajectory grants no new DSP-history authority.
        let continuing = entry.output_gain == old.output_gain
            && (entry.bypassed == old.bypassed
                || (entry.algorithmic_latency_frames == Some(0)
                    && old.algorithmic_latency_frames == Some(0)
                    && entry.supports_smoothed_bypass
                    && old.supports_smoothed_bypass));
        let gain_edit = !continuing
            && !entry.bypassed
            && !old.bypassed
            && entry.input_gain == old.input_gain
            && entry.matrix == old.matrix
            && entry.automation == old.automation
            && match (&entry.configuration, &old.configuration) {
                (Some(a), Some(b)) => {
                    std::sync::Arc::ptr_eq(a, b) || a.processing_eq_on_callback(b)
                }
                _ => false,
            };
        let eligible = !excluded.contains(&index)
            && configuration_present
            && (gain_edit || continuing)
            && !entry.automation_frozen
            && !old.automation_frozen
            && !entry.automation.has_host_target(HostOutputGain)
            && !old.automation.has_host_target(HostOutputGain)
            && entry.automation.input_target() != Some(HostOutputGain)
            && old.automation.input_target() != Some(HostOutputGain)
            && !matches!(entry.runtime, RuntimeOwnership::Foreign(_))
            && !matches!(old.runtime, RuntimeOwnership::Foreign(_))
            && entry.runtime.can_inherit_from(&old.runtime);
        ramp = OutputGainRamp::inherited(
            old.committed_output_gain_ramp,
            old.output_gain,
            old.committed_host_gains[1],
            entry.output_gain,
            entry.sample_rate,
            eligible,
        );
        if eligible {
            entry.committed_host_gains[1] = old.committed_host_gains[1];
            entry.scratch_host_gains[1] = old.committed_host_gains[1];
        }
    }
    entry.committed_output_gain_ramp = ramp;
    entry.scratch_output_gain_ramp = ramp;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_endpoint_and_partition_independent_pcm_at_supported_rates() {
        for rate in [8000_u32, 44100, 48000, 96000, 192000] {
            let total = rate.div_ceil(200) as usize;
            let expected = (1..=total + 17)
                .map(|i| {
                    let gain = if i >= total {
                        0.25
                    } else {
                        (1.0 - 0.75 * i as f64 / total as f64) as f32
                    };
                    [gain * 0.5, -gain * 0.25]
                })
                .collect::<Vec<_>>();
            for block in [1, 7, 64, 512] {
                let mut ramp =
                    OutputGainRamp::inherited(OutputGainRamp::INACTIVE, 1.0, 1.0, 0.25, rate, true);
                let mut pcm = vec![[0.5, -0.25]; total + 17];
                for chunk in pcm.chunks_mut(block) {
                    ramp.apply(0.25, chunk);
                }
                assert_eq!(pcm, expected, "rate{rate} block{block}");
                assert!(!ramp.active());
                assert_eq!(ramp.apply(0.25, &mut []), None);
            }
        }
    }
    #[test]
    fn unchanged_handoffs_continue_and_reversal_starts_from_committed_effective_gain() {
        let mut ramp =
            OutputGainRamp::inherited(OutputGainRamp::INACTIVE, 1.0, 1.0, 0.25, 48000, true);
        let gain = ramp.apply(0.25, &mut [[1.0; 2]; 73]).unwrap();
        assert_eq!(
            OutputGainRamp::inherited(ramp, 0.25, gain, 0.25, 48000, true),
            ramp
        );
        let mut reverse = OutputGainRamp::inherited(ramp, 0.25, gain, 1.0, 48000, true);
        let mut pcm = [[1.0; 2]; 240];
        reverse.apply(1.0, &mut pcm);
        assert!((pcm[0][0] - gain).abs() < 0.005);
        assert!(pcm.windows(2).all(|p| p[0][0] <= p[1][0]));
        assert_eq!(pcm[239], [1.0; 2]);
        assert_eq!(
            OutputGainRamp::inherited(ramp, 0.25, gain, 1.0, 48000, false),
            OutputGainRamp::INACTIVE
        );
        for invalid in [f32::NAN, f32::INFINITY, -1.0] {
            assert_eq!(
                OutputGainRamp::inherited(ramp, 0.25, invalid, 1.0, 48000, true),
                OutputGainRamp::INACTIVE
            );
        }
    }
}

#[cfg(test)]
mod chain_tests;
