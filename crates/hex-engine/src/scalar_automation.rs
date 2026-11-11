//! Preparation declarations and bounded materialization for recorded native
//! parameters. Source/catalog custody remains with the stream owner. Admission
//! is one exact native effect; delayed stages use the compensation owner.
use crate::{Invalid, MediaPages, ScalarSourceMetadata, ScalarSourceTarget};
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

/// Limit the combined page and stage work of one recording playback window.
pub const MAX_SCALAR_AUTOMATION_BINDINGS: usize = 16;

#[derive(Debug)]
pub(crate) struct PreparedBinding {
    pub(crate) request: ScalarAutomationBinding,
    pub(crate) input: crate::NativeParameterInputTarget,
    pub(crate) arrival_frames: u32,
}

pub(crate) fn validate_source(
    project: &hex_project::Project,
    binding: &ScalarAutomationBinding,
) -> Result<(), Invalid> {
    let source = &binding.source;
    if source.target()
        != (ScalarSourceTarget::Device {
            device: binding.device,
            target: binding.target,
        })
    {
        return Err(Invalid("scalar take target does not match its binding"));
    }
    if source.project_rate() != project.sample_rate()
        || !projection_matches(source, project.tempo_map())
    {
        return Err(Invalid(
            "scalar automation source rate or projection differs from project",
        ));
    }
    // Recorded sources are not yet owned by canonical lane state. Until that
    // adoption can preserve them while switching read modes, refuse this
    // binding rather than silently play over an existing manual override.
    if project
        .device(binding.device)
        .and_then(|device| device.automation_target(binding.target))
        .is_some_and(|lane| !lane.is_reading())
    {
        return Err(Invalid("recorded automation target is manually overridden"));
    }
    Ok(())
}
pub(crate) fn projection_matches(
    source: &ScalarSourceMetadata,
    map: &hex_project::TempoMap,
) -> bool {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hex_project::visit_automation_projection_identity(map, &mut |bytes| hash.update(bytes));
    let fingerprint: [u8; 32] = hash.finalize().into();
    source.projection_identity().fingerprint == fingerprint
}

/// Group a bounded stage-origin window into contiguous source-frame runs.
/// Missing origins never request pages. Both readiness and materialization use
/// this partition so a discontinuity cannot turn into an accidental long read.
pub(crate) fn visit_origin_runs(
    frames: usize,
    mut origin: impl FnMut(usize) -> Result<Option<u64>, Invalid>,
    mut visit: impl FnMut(usize, u64, usize) -> Result<(), Invalid>,
) -> Result<(), Invalid> {
    if frames > 256 {
        return Err(Invalid("scalar origin window exceeds its fixed quantum"));
    }
    let mut run: Option<(usize, u64, usize)> = None;
    for offset in 0..frames {
        let frame = origin(offset)?;
        if let (Some((_, start, len)), Some(frame)) = (run, frame)
            && start.checked_add(len as u64) == Some(frame)
        {
            if let Some((_, _, length)) = &mut run {
                *length += 1;
            }
            continue;
        }
        if let Some((start, frame, len)) = run.take() {
            visit(start, frame, len)?;
        }
        if let Some(frame) = frame {
            run = Some((offset, frame, 1));
        }
    }
    if let Some((start, frame, len)) = run {
        visit(start, frame, len)?;
    }
    Ok(())
}

/// Fixed preparation-allocated scratch. Filling completes before any PCM/DSP
/// access for this chunk; failed materialization never becomes a base fallback.
struct State {
    raw: Vec<Option<hex_project::AutomationValue>>,
    values: Vec<Option<NormalizedAutomationValue>>,
    frames: usize,
    arrival_frames: u32,
}
impl State {
    /// Fixed DATA reservation, independently checked against actual capacities.
    /// Empty-to-fixed vectors have no reallocation overlap; the optional Box is
    /// only this small owner, never a large constructor temporary on the stack.
    pub(crate) const fn requested_data_bytes() -> usize {
        std::mem::size_of::<Self>()
            + 256
                * (std::mem::size_of::<Option<hex_project::AutomationValue>>()
                    + std::mem::size_of::<Option<NormalizedAutomationValue>>())
    }
    fn new(arrival_frames: u32) -> Result<Self, Invalid> {
        fn scratch<T: Copy>() -> Result<Vec<Option<T>>, Invalid> {
            let mut values = Vec::new();
            values
                .try_reserve_exact(256)
                .map_err(|_| Invalid("scalar scratch allocation failed"))?;
            if values.capacity() != 256 {
                return Err(Invalid("scalar scratch exceeds its fixed data reservation"));
            }
            values.resize(256, None);
            Ok(values)
        }
        Ok(Self {
            raw: scratch()?,
            values: scratch()?,
            frames: 0,
            arrival_frames,
        })
    }
    pub(crate) fn arrival_frames(&self) -> u32 {
        self.arrival_frames
    }
    pub(crate) fn fill_with_origins<P: MediaPages + ?Sized>(
        &mut self,
        pages: &mut P,
        source: &ScalarSourceMetadata,
        frames: usize,
        origin: impl FnMut(usize) -> Result<Option<u64>, Invalid>,
    ) -> Result<(), Invalid> {
        self.frames = 0;
        self.values.fill(None);
        let raw = self
            .raw
            .get_mut(..frames)
            .ok_or(Invalid("scalar automation chunk exceeds scratch"))?;
        raw.fill(None);
        visit_origin_runs(frames, origin, |start, frame, len| {
            let end = start
                .checked_add(len)
                .ok_or(Invalid("scalar origin run overflow"))?;
            let destination = raw
                .get_mut(start..end)
                .ok_or(Invalid("scalar origin run is outside scratch"))?;
            crate::read_scalar_values(pages, source, frame, destination)
        })?;
        self.finish_fill(frames)
    }
    fn finish_fill(&mut self, frames: usize) -> Result<(), Invalid> {
        let raw = self
            .raw
            .get(..frames)
            .ok_or(Invalid("scalar automation chunk exceeds scratch"))?;
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
        Ok(())
    }
    fn value_at(&self, offset: usize) -> Option<NormalizedAutomationValue> {
        self.values
            .get(offset)
            .copied()
            .flatten()
            .filter(|_| offset < self.frames)
    }
}

/// One fully materialized window per exact processor. A group borrows its
/// preallocated scalar lanes through the existing shared parameter contract.
pub(crate) struct StateSet {
    groups: Vec<Group>,
    offset: usize,
}

struct Group {
    identity: hex_lattice::ProcessorInstanceIdentity,
    targets: Vec<DeviceAutomationTarget>,
    states: Vec<State>,
}

impl StateSet {
    pub(crate) fn requested_data_bytes(bindings: &[PreparedBinding]) -> Option<usize> {
        let groups = groups_by_identity(bindings).count();
        std::mem::size_of::<Self>()
            .checked_add(groups.checked_mul(std::mem::size_of::<Group>())?)?
            .checked_add(bindings.len().checked_mul(
                State::requested_data_bytes() + std::mem::size_of::<DeviceAutomationTarget>(),
            )?)
    }

    pub(crate) fn new(bindings: &[PreparedBinding]) -> Result<Self, Invalid> {
        let mut groups = fixed_vec(groups_by_identity(bindings).count())?;
        for slice in groups_by_identity(bindings) {
            let mut targets = fixed_vec(slice.len())?;
            let mut states = fixed_vec(slice.len())?;
            for binding in slice {
                targets.push(binding.input.target());
                states.push(State::new(binding.arrival_frames)?);
            }
            groups.push(Group {
                identity: slice[0].input.identity(),
                targets,
                states,
            });
        }
        Ok(Self { groups, offset: 0 })
    }

    pub(crate) fn fill<P: MediaPages + ?Sized>(
        &mut self,
        bindings: &[PreparedBinding],
        pages: &mut P,
        frames: usize,
        offset: usize,
        mut origin: impl FnMut(u32, usize) -> Result<Option<u64>, Invalid>,
    ) -> Result<(), Invalid> {
        for (state, binding) in self
            .groups
            .iter_mut()
            .flat_map(|group| &mut group.states)
            .zip(bindings)
        {
            let delay = state.arrival_frames();
            state.fill_with_origins(pages, &binding.request.source, frames, |index| {
                origin(delay, index)
            })?;
        }
        self.offset = offset;
        Ok(())
    }
}

fn groups_by_identity(
    bindings: &[PreparedBinding],
) -> impl Iterator<Item = &[PreparedBinding]> + Clone {
    bindings.chunk_by(|a, b| a.input.identity() == b.input.identity())
}

fn fixed_vec<T>(capacity: usize) -> Result<Vec<T>, Invalid> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| Invalid("scalar group allocation failed"))?;
    if values.capacity() != capacity {
        return Err(Invalid("scalar group exceeds its data reservation"));
    }
    Ok(values)
}

impl hex_lattice::NativeExternalAutomationSet for StateSet {
    fn window(
        &self,
        identity: hex_lattice::ProcessorInstanceIdentity,
    ) -> Option<NativeExternalAutomation<'_>> {
        let group = self
            .groups
            .iter()
            .find(|group| group.identity == identity)?;
        Some(NativeExternalAutomation {
            identity,
            frame_offset: self.offset,
            values: group,
        })
    }
}

impl ExternalParameterAutomation for Group {
    fn frames(&self) -> usize {
        self.states.first().map_or(0, |state| state.frames)
    }

    fn targets(&self) -> &[DeviceAutomationTarget] {
        &self.targets
    }

    fn value_at(
        &self,
        target: DeviceAutomationTarget,
        offset: usize,
    ) -> Option<NormalizedAutomationValue> {
        let index = self.targets.binary_search(&target).ok()?;
        self.states[index].value_at(offset)
    }
}

#[cfg(test)]
mod tests;
