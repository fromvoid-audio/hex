//! Complete immutable granular inputs retained by ordinary control-side hosts.
//! Requirements carry no PCM, reader, worker, page loan or publication authority.
use super::*;

#[derive(Clone)]
pub struct StreamedGranularRequirement {
    inner: Arc<Requirement>,
}
struct Requirement {
    raw: crate::StreamedSourceMetadata,
    root: StreamedGranularRoot,
}
impl StreamedGranularRequirement {
    /// Capture the existing canonical root. The caller admits bounded metadata
    /// construction and binds the raw asset to its verified file on control.
    pub fn new(
        raw: &crate::StreamedSourceMetadata,
        edit: &AudioClipEdit,
        tempo: &hex_project::TempoMap,
        cancel: &AtomicBool,
    ) -> Result<Self, Invalid> {
        let root = streamed_granular_root(raw, edit, tempo, cancel)?;
        Ok(Self {
            inner: Arc::new(Requirement {
                raw: raw.clone(),
                root,
            }),
        })
    }
    pub fn asset(&self) -> hex_project::AssetId {
        self.inner.raw.key().asset()
    }
    /// Rebind only the catalog generation; all captured source facts must match.
    pub fn metadata_for(
        &self,
        raw: &crate::StreamedSourceMetadata,
    ) -> Result<crate::StreamedSourceMetadata, Invalid> {
        let expected = &self.inner.raw;
        if !raw.key().is_raw()
            || raw.key().asset() != expected.key().asset()
            || raw.source_rate() != expected.source_rate()
            || raw.project_rate() != expected.project_rate()
            || raw.source_frames() != expected.source_frames()
            || raw.playback_frames() != expected.playback_frames()
            || raw.conversion_version() != expected.conversion_version()
            || raw.fingerprint() != expected.fingerprint()
        {
            return Err(Invalid("granular requirement source facts differ"));
        }
        let root = &self.inner.root;
        raw.with_granular(
            &root.region,
            &root.map,
            root.mode,
            root.pitch_cents,
            root.law,
        )
    }
    /// Listed DATA bound for an independent root copy, excluding the retained
    /// shared requirement, allocator metadata and subsequent worker processing.
    /// The caller reserves this before try_owned_root on its control worker.
    pub fn owned_root_bytes(&self) -> Option<usize> {
        std::mem::size_of::<StreamedGranularRoot>().checked_add(self.inner.root.backing_bytes()?)
    }
    /// Fallibly copy the complete root for one independently owned worker.
    /// The original shared owner stays intact on refusal or cancellation.
    pub fn try_owned_root(
        &self,
        raw: &crate::StreamedSourceMetadata,
        cancel: &AtomicBool,
    ) -> Result<StreamedGranularRoot, Invalid> {
        if cancel.load(Ordering::Acquire) {
            return Err(Invalid("audio preparation cancelled"));
        }
        let metadata = self.metadata_for(raw)?;
        let root = &self.inner.root;
        let mut envelope = Vec::new();
        envelope
            .try_reserve_exact(root.region.envelope().len())
            .map_err(|_| Invalid("granular envelope reservation failed"))?;
        envelope.extend_from_slice(root.region.envelope());
        let region = hex_warp::SourceRegion::new(
            root.region.source_frames(),
            root.region.start(),
            root.region.length(),
            root.region.phase(),
            root.region.looping(),
            envelope,
        )
        .map_err(warp_error)?;
        let mut segments = Vec::new();
        segments
            .try_reserve_exact(root.map.segments().len())
            .map_err(|_| Invalid("granular map reservation failed"))?;
        segments.extend_from_slice(root.map.segments());
        let map = hex_warp::TimeMap::new_streamed(
            segments,
            root.map
                .segments()
                .last()
                .ok_or(Invalid("empty granular map"))?
                .output_end,
        )
        .map_err(warp_error)?;
        let copied = StreamedGranularRoot {
            metadata,
            region,
            map,
            mode: root.mode,
            pitch_cents: root.pitch_cents,
            law: root.law,
        };
        if cancel.load(Ordering::Acquire) {
            return Err(Invalid("audio preparation cancelled"));
        }
        if copied
            .backing_bytes()
            .is_none_or(|bytes| root.backing_bytes().is_none_or(|bound| bytes > bound))
        {
            return Err(Invalid("granular root copy reservation exceeded"));
        }
        Ok(copied)
    }
    /// Actual shared owner payload, Arc counters and region/map allocations.
    /// Caller handle/vector DATA and opaque allocator metadata are separate.
    pub fn retained_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Requirement>()
            .checked_add(2 * std::mem::size_of::<usize>())?
            .checked_add(self.inner.root.backing_bytes()?)
    }
}

/// Bounded control-owned requirement handles. Cloned handles share complete
/// region/map owners; cloned sets retain their own actual vector backing.
#[derive(Clone, Default)]
pub struct StreamedGranularRequirements {
    requirements: Vec<StreamedGranularRequirement>,
}
impl StreamedGranularRequirements {
    pub fn new(requirements: Vec<StreamedGranularRequirement>) -> Result<Self, Invalid> {
        if requirements.len() > hex_project::MAX_CLIPS
            || requirements.capacity() > hex_project::MAX_CLIPS
        {
            return Err(Invalid("granular requirement count exceeded"));
        }
        Ok(Self { requirements })
    }
    pub fn try_push(&mut self, requirement: StreamedGranularRequirement) -> Result<(), Invalid> {
        if self.requirements.len() >= hex_project::MAX_CLIPS {
            return Err(Invalid("granular requirement count exceeded"));
        }
        self.requirements
            .try_reserve_exact(1)
            .map_err(|_| Invalid("granular requirement reservation failed"))?;
        if self.requirements.capacity() > hex_project::MAX_CLIPS {
            return Err(Invalid("granular requirement capacity exceeded"));
        }
        self.requirements.push(requirement);
        Ok(())
    }
    pub fn handle_capacity_bytes(&self) -> Option<usize> {
        self.requirements
            .capacity()
            .checked_mul(std::mem::size_of::<StreamedGranularRequirement>())
    }
    pub fn requirements(&self) -> &[StreamedGranularRequirement] {
        &self.requirements
    }
    pub(crate) fn visit<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        use crate::audio_storage::{Location, Owner};
        ledger.bytes(
            Owner::exclusive(self),
            Location("granular-requirement-set", 0),
            std::mem::size_of::<Self>(),
        )?;
        ledger.vec(&self.requirements)?;
        for requirement in &self.requirements {
            ledger.bytes(
                Owner::shared(&requirement.inner),
                Location("granular-requirement-root", 0),
                requirement
                    .retained_bytes()
                    .ok_or(crate::AudioStorageError::Overflow)?,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
