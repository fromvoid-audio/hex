//! Atomic analyzer membership for one visible device chain. Stable slots keep
//! independent rolling windows when focus or card order changes. The 64-target
//! bound matches the canonical chain limit, checked by the owning contract test.
use super::{PostNodeIdentity, ProcessorChainOwner, SpectrumTarget, SpectrumTargetId, TargetError};

pub const MAX_SPECTRUM_TARGETS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpectrumTargetRequest {
    pub identity: PostNodeIdentity,
    pub project_generation: u64,
    pub activation_floor: u64,
    pub sample_rate_hz: u32,
}
impl From<SpectrumTarget> for SpectrumTargetRequest {
    fn from(target: SpectrumTarget) -> Self {
        Self {
            identity: target.identity,
            project_generation: target.project_generation,
            activation_floor: target.activation_floor,
            sample_rate_hz: target.sample_rate_hz,
        }
    }
}
impl SpectrumTargetRequest {
    fn validate(self) -> Result<(), TargetError> {
        if self.identity.device_id == 0 {
            return Err(TargetError::InvalidDevice);
        }
        if matches!(
            self.identity.owner,
            ProcessorChainOwner::Track(0) | ProcessorChainOwner::Return(0)
        ) {
            return Err(TargetError::InvalidOwner);
        }
        if !matches!(self.identity.device_type.as_str(), Ok("hex.eq8" | "hex.eq")) {
            return Err(TargetError::WrongDeviceType);
        }
        if self.activation_floor == 0 {
            return Err(TargetError::InvalidActivationFloor);
        }
        if !(8_000..=192_000).contains(&self.sample_rate_hz) {
            return Err(TargetError::UnsupportedSampleRate(self.sample_rate_hz));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpectrumTargets {
    slots: [Option<SpectrumTarget>; MAX_SPECTRUM_TARGETS],
    lookup: [(u64, usize); MAX_SPECTRUM_TARGETS],
    len: usize,
}
impl SpectrumTargets {
    pub const fn empty() -> Self {
        Self {
            slots: [None; MAX_SPECTRUM_TARGETS],
            lookup: [(0, 0); MAX_SPECTRUM_TARGETS],
            len: 0,
        }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn iter(&self) -> impl Iterator<Item = (usize, SpectrumTarget)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.map(|t| (i, t)))
    }
    pub fn get(&self, slot: usize) -> Option<SpectrumTarget> {
        self.slots.get(slot).copied().flatten()
    }
    pub fn find(&self, identity: PostNodeIdentity) -> Option<(usize, SpectrumTarget)> {
        let lookup = self.lookup.get(..self.len)?;
        let index = lookup
            .binary_search_by_key(&identity.device_id, |entry| entry.0)
            .ok()?;
        let slot = lookup.get(index)?.1;
        self.get(slot)
            .filter(|t| t.identity == identity)
            .map(|t| (slot, t))
    }
    pub(super) fn replacing(
        &self,
        requests: &[SpectrumTargetRequest],
        mut next_token: u64,
    ) -> Result<(Self, u64), TargetError> {
        if requests.len() > MAX_SPECTRUM_TARGETS {
            return Err(TargetError::TooManyTargets);
        }
        for (index, request) in requests.iter().enumerate() {
            request.validate()?;
            if requests[..index]
                .iter()
                .any(|other| other.identity.device_id == request.identity.device_id)
            {
                return Err(TargetError::DuplicateTarget);
            }
            if requests.first().is_some_and(|first| {
                first.project_generation != request.project_generation
                    || first.sample_rate_hz != request.sample_rate_hz
            }) {
                return Err(TargetError::InconsistentContext);
            }
        }
        let mut next = Self::empty();
        for (slot, target) in self.iter() {
            if requests.contains(&SpectrumTargetRequest::from(target))
                && let Some(entry) = next.slots.get_mut(slot)
            {
                *entry = Some(target);
            }
        }
        for request in requests {
            if next.find_request(*request).is_some() {
                continue;
            }
            let token = next_token;
            next_token = next_token
                .checked_add(1)
                .ok_or(TargetError::TokenExhausted)?;
            let Some(entry) = next.slots.iter_mut().find(|entry| entry.is_none()) else {
                return Err(TargetError::TooManyTargets);
            };
            *entry = Some(SpectrumTarget {
                token: SpectrumTargetId(token),
                identity: request.identity,
                project_generation: request.project_generation,
                activation_floor: request.activation_floor,
                sample_rate_hz: request.sample_rate_hz,
            });
        }
        for (slot, target) in next.slots.iter().enumerate() {
            if let Some(target) = target {
                let Some(entry) = next.lookup.get_mut(next.len) else {
                    return Err(TargetError::TooManyTargets);
                };
                *entry = (target.identity.device_id, slot);
                next.len += 1;
            }
        }
        if let Some(lookup) = next.lookup.get_mut(..next.len) {
            lookup.sort_unstable_by_key(|entry| entry.0);
        }
        Ok((next, next_token))
    }
    fn find_request(&self, request: SpectrumTargetRequest) -> Option<SpectrumTarget> {
        self.iter()
            .map(|(_, target)| target)
            .find(|target| SpectrumTargetRequest::from(*target) == request)
    }
}
