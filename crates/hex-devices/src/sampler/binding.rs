//! Source binding for the production sampler adapter. No owning PCM or lease is
//! stored here: the host must retain the admitted configuration through rendering.
use super::*;
use hex_lattice::{DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView, DeviceResources};
use hex_lattice_contract::DeviceAssetSlot;

#[derive(Clone, Copy, Debug)]
struct SourceKey {
    slot: DeviceAssetSlot,
    identity: DeviceResourceIdentity,
    shape: SampleShape,
}
impl SourceKey {
    fn matches(self, view: DeviceResourceView<'_>) -> bool {
        self.slot == view.slot
            && self.identity == view.identity
            && self.shape.sample_rate() == view.sample_rate
            && self.shape.frames() == view.pcm.frames()
            && self.shape.channels() == channels(view.pcm)
    }
}
fn channels(pcm: DeviceResourcePcm<'_>) -> SampleChannels {
    match pcm {
        DeviceResourcePcm::Mono(_) => SampleChannels::Mono,
        DeviceResourcePcm::Stereo(_) => SampleChannels::Stereo,
    }
}

/// Control-only plan with a checked source binding. Markers remain borrowed
/// only during preparation. Numeric/source identity checks grant no ownership,
/// storage provenance, publication or retirement authority.
#[derive(Clone, Copy, Debug)]
pub struct BoundSamplerPlan<'a> {
    plan: PreparedSamplerPlan<'a>,
    source: SourceKey,
}
impl<'a> PreparedSamplerPlan<'a> {
    /// Bind and validate finite PCM once on control, never once per note or in
    /// the callback. The caller must provide its actually retained immutable
    /// source configuration; replacing it requires a newly prepared binding.
    pub fn bind_source_on_control(
        self,
        view: DeviceResourceView<'_>,
    ) -> Result<BoundSamplerPlan<'a>, Invalid> {
        let source = SourceKey {
            slot: view.slot,
            identity: view.identity,
            shape: self.sample,
        };
        if view.identity.derivation_version == 0 || !source.matches(view) {
            return Err(Invalid(
                "sampler source binding does not match the prepared plan",
            ));
        }
        let finite = match view.pcm {
            DeviceResourcePcm::Mono(pcm) => pcm.iter().all(|value| value.is_finite()),
            DeviceResourcePcm::Stereo(pcm) => pcm.iter().flatten().all(|value| value.is_finite()),
        };
        if !finite {
            return Err(Invalid("sampler source needs finite pcm"));
        }
        Ok(BoundSamplerPlan { plan: self, source })
    }
}
impl BoundSamplerPlan<'_> {
    /// Produces only scalar coefficients and source identity, without retaining
    /// source buffers or marker storage. Polyphony and root custody stay in host.
    pub fn prepare_voice(
        self,
        note: u8,
        velocity: u8,
    ) -> Result<Option<BorrowedSamplerVoice>, Invalid> {
        Ok(self
            .plan
            .prepare_voice_coefficients(note, velocity)?
            .map(|coefficients| BorrowedSamplerVoice {
                coefficients,
                source: self.source,
            }))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BorrowedSamplerVoice {
    coefficients: VoiceCoefficients,
    source: SourceKey,
}
impl BorrowedSamplerVoice {
    pub fn end_age_frames(&self, gate_frames: u64) -> Result<u64, Invalid> {
        self.coefficients.end_age_frames(gate_frames)
    }
    pub fn release_frames(&self) -> u64 {
        self.coefficients.release_frames()
    }
    /// Query the actual source configuration, including carried/outgoing voices.
    /// A missing or changed source is silence, never a fallback to another slot.
    pub fn sample(&self, resources: &dyn DeviceResources, age: u64, gate: u64) -> StereoFrame {
        resources
            .slot(self.source.slot)
            .filter(|view| self.source.matches(*view))
            .map_or([0.0; 2], |view| {
                self.coefficients.sample(view.pcm, age, gate)
            })
    }
}
