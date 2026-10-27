//! Preparation declarations and bounded materialization for recorded native
//! parameters. Source/catalog custody remains with the stream owner. The first
//! admission is one exact zero-latency native effect; no saved fields are added.
use crate::{Invalid, MediaPages, ScalarSourceMetadata};
use hex_lattice::{
    ExternalParameterAutomation, NativeExternalAutomation, NormalizedAutomationValue,
};
use hex_lattice_contract::DeviceAutomationTarget;

/// Control-side requested source binding, validated against the canonical
/// effect and the actual stream catalog before native publication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalarAutomationBinding {
    pub device: hex_project::DeviceId,
    pub target: DeviceAutomationTarget,
    pub source: ScalarSourceMetadata,
}

pub(crate) fn projection_matches(source: &ScalarSourceMetadata, map: &hex_project::TempoMap) -> bool {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hex_project::visit_automation_projection_identity(map, &mut |bytes| hash.update(bytes));
    let fingerprint: [u8; 32] = hash.finalize().into();
    source.projection_identity().fingerprint == fingerprint
}

/// Fixed preparation-allocated scratch. Filling completes before any PCM/DSP
/// access for this chunk; failed materialization never becomes a base fallback.
pub(crate) struct State {
    target: [DeviceAutomationTarget; 1],
    raw: [Option<hex_project::AutomationValue>; 256],
    values: [Option<NormalizedAutomationValue>; 256],
    frames: usize,
    offset: usize,
    identity: hex_lattice::ProcessorInstanceIdentity,
}
impl State {
    pub(crate) fn new(input: crate::NativeParameterInputTarget) -> Self {
        Self {
            target: [input.target()],
            raw: [None; 256],
            values: [None; 256],
            frames: 0,
            offset: 0,
            identity: input.identity(),
        }
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.frames == 0
    }
    pub(crate) fn fill<P: MediaPages + ?Sized>(
        &mut self,
        pages: &mut P,
        source: &ScalarSourceMetadata,
        start: u64,
        frames: usize,
        offset: usize,
    ) -> Result<(), Invalid> {
        self.frames = 0;
        self.values.fill(None);
        let raw = self
            .raw
            .get_mut(..frames)
            .ok_or(Invalid("scalar automation chunk exceeds scratch"))?;
        crate::read_scalar_values(pages, source, start, raw)?;
        for (slot, value) in self.values.iter_mut().zip(raw) {
            *slot = match value {
                Some(value) => Some(
                    NormalizedAutomationValue::new(value.get())
                        .map_err(|_| Invalid("scalar automation value is invalid"))?,
                ),
                None => None,
            };
        }
        self.frames = frames;
        self.offset = offset;
        Ok(())
    }
    pub(crate) fn window(&self) -> NativeExternalAutomation<'_> {
        NativeExternalAutomation {
            identity: self.identity,
            frame_offset: self.offset,
            values: self,
        }
    }
}
impl ExternalParameterAutomation for State {
    fn frames(&self) -> usize {
        self.frames
    }
    fn targets(&self) -> &[DeviceAutomationTarget] {
        &self.target
    }
    fn value_at(
        &self,
        target: DeviceAutomationTarget,
        offset: usize,
    ) -> Option<NormalizedAutomationValue> {
        if self.target[0] != target || offset >= self.frames {
            return None;
        }
        self.values.get(offset).copied().flatten()
    }
}
