//! Actual audio-only preview owner, retained in the existing preparation root.
//! Constructor/layout observations leave its allocation/handle obligation OPEN.
use super::{MidiArrangementPreview, MidiClipPreview};
use crate::{AudioStorageError as Error, owned_shared::OwnedShared};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioPreviewCapacity {
    pub clips_payload_bytes: usize,
    pub clips_declared_overhead_bytes: usize,
    pub reserved_handle_destinations: usize,
    pub independent_handle_obligations: usize,
    pub unresolved_allocations: usize,
}
impl AudioPreviewCapacity {
    pub(crate) fn bytes(self) -> Result<usize, Error> {
        self.clips_payload_bytes
            .checked_add(self.clips_declared_overhead_bytes)
            .ok_or(Error::Overflow)
    }
}
/// Copyable leaf metadata, not a legacy preview/clip or owning child reference.
/// ```compile_fail
/// fn escape(v: hex_engine::AudioPreviewMetadata) { let _ = v.clips(); }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioPreviewMetadata {
    pub clip_count: usize,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Vacant,
    Reserved,
    Installed,
    Cleaning,
    Complete,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Disposal {
    #[default]
    Unstarted,
    Pending,
    Complete,
}
#[derive(Default)]
pub(crate) struct PreviewCustody {
    clips: Option<OwnedShared<[MidiClipPreview]>>,
    preview: Option<MidiArrangementPreview>,
    handle: u64,
    phase: Phase,
    disposal: Disposal,
    #[cfg(test)]
    pub(crate) fail_after: Option<usize>,
    #[cfg(test)]
    pub(crate) panic_after: Option<usize>,
    #[cfg(test)]
    pub(crate) probe: Option<crate::audio_preparation::DropProbe>,
}
impl PreviewCustody {
    pub(crate) fn capacity(&self) -> Result<AudioPreviewCapacity, Error> {
        self.capacity_at(None)
    }
    pub(crate) fn can_transfer_revision(&self) -> bool {
        self.phase == Phase::Installed && self.preview.is_some() && self.clips.is_none()
    }
    pub(crate) fn can_restore_revision(&self) -> bool {
        self.phase == Phase::Installed
            && self.preview.is_none()
            && self.clips.is_none()
            && self.disposal == Disposal::Unstarted
    }
    pub(crate) fn transfer_revision(&mut self, target: &mut Option<MidiArrangementPreview>) {
        *target = self.preview.take();
    }
    pub(crate) fn restore_revision(&mut self, source: &mut Option<MidiArrangementPreview>) {
        self.preview = source.take();
    }
    pub(crate) fn capacity_at(
        &self,
        external: Option<&MidiArrangementPreview>,
    ) -> Result<AudioPreviewCapacity, Error> {
        if self.preview.is_some() && external.is_some() {
            return Err(Error::SourceMismatch);
        }
        if self.clips.is_some() && (self.preview.is_some() || external.is_some()) {
            return Err(Error::UnknownStorage);
        }
        let owner = self.clips.as_ref().or_else(|| {
            self.preview
                .as_ref()
                .or(external)
                .map(|MidiArrangementPreview { clips }| clips)
        });
        let mut report = AudioPreviewCapacity {
            reserved_handle_destinations: usize::from(self.handle != 0),
            ..Default::default()
        };
        if let Some(owner) = owner {
            if self.handle == 0 || !owner.is_empty() {
                return Err(Error::UnknownStorage);
            }
            let (payload, overhead) = owner.parts().ok_or(Error::UnsupportedConstructor)?;
            report.clips_payload_bytes = payload;
            report.clips_declared_overhead_bytes = overhead;
            report.independent_handle_obligations = 1;
            report.unresolved_allocations = 1;
        }
        report.bytes()?;
        Ok(report)
    }
    pub(crate) fn reserve(&mut self, project: &hex_project::Project) -> Result<(), Error> {
        self.reserve_with_current_sampler(project, false, false)
    }
    /// The bounded native CURRENT class uses canonical UI note geometry. This
    /// private empty analytic preview is not a second source or note renderer.
    pub(crate) fn reserve_with_current_sampler(
        &mut self,
        project: &hex_project::Project,
        sampler: bool,
        effects: bool,
    ) -> Result<(), Error> {
        if self.phase != Phase::Vacant
            || self.clips.is_some()
            || self.preview.is_some()
            || self.handle != 0
            || self.disposal != Disposal::Unstarted
        {
            return Err(Error::Capacity);
        }
        if sampler {
            if !crate::native_device_resources::current_sampler::validate_project_with_effects(
                project, effects,
            )? {
                return Err(Error::UnsupportedGraph);
            }
        } else if project.tracks().iter().any(|t| !t.midi_clips().is_empty()) {
            return Err(Error::UnsupportedGraph);
        }
        if !crate::pcm_arc::native_layout_known() {
            return Err(Error::UnsupportedConstructor);
        }
        // Checked independent handle and vacant persistent destination precede
        // the explicit sized-zero-array allocation. No raw/default adoption.
        self.handle = crate::audio_only_inputs::handle()?;
        self.phase = Phase::Reserved;
        self.clips = Some(OwnedShared::new_owned_empty_slice());
        self.capacity()?;
        #[cfg(test)]
        self.fault(0)?;
        Ok(())
    }
    pub(crate) fn construct(&mut self) -> Result<(), Error> {
        if self.phase != Phase::Reserved
            || self.preview.is_some()
            || self.disposal != Disposal::Unstarted
            || self.capacity()?.independent_handle_obligations != 1
        {
            return Err(Error::Capacity);
        }
        // The exact slice+declaration travels inside the actual preview value;
        // no allocation, clone, owning error or fallible step between moves.
        if let Some(clips) = self.clips.take() {
            self.preview = Some(MidiArrangementPreview { clips });
        }
        self.phase = Phase::Installed;
        #[cfg(test)]
        self.fault(1)?;
        Ok(())
    }
    #[cfg(test)]
    fn fault(&self, phase: usize) -> Result<(), Error> {
        assert_ne!(self.panic_after, Some(phase), "persistent preview panic");
        if self.fail_after == Some(phase) {
            return Err(Error::Allocation);
        }
        Ok(())
    }
    pub(crate) fn metadata_at(
        &self,
        external: Option<&MidiArrangementPreview>,
    ) -> Option<AudioPreviewMetadata> {
        if self.phase != Phase::Installed {
            return None;
        }
        if self.preview.is_some() && external.is_some() {
            return None;
        }
        self.preview
            .as_ref()
            .or(external)
            .map(|p| AudioPreviewMetadata {
                clip_count: p.clips.len(),
            })
    }
    pub(crate) fn has_next(&self) -> bool {
        !matches!(self.phase, Phase::Vacant | Phase::Complete)
    }
    pub(crate) fn dispose_next(&mut self) -> Result<(), Error> {
        self.phase = Phase::Cleaning;
        if self.preview.is_some() {
            // Validate the vacant persistent destination BEFORE taking owner.
            if self.clips.is_some() || self.disposal != Disposal::Unstarted {
                return Err(Error::Capacity);
            }
            self.capacity()?;
            if let Some(MidiArrangementPreview { clips }) = self.preview.take() {
                self.clips = Some(clips);
            }
            return Ok(());
        }
        if self.disposal != Disposal::Unstarted {
            return Err(Error::UnknownStorage);
        }
        self.capacity()?;
        self.disposal = Disposal::Pending;
        drop(self.clips.take());
        #[cfg(test)]
        drop(self.probe.take());
        self.disposal = Disposal::Complete;
        self.phase = Phase::Complete;
        Ok(())
    }
    pub(crate) fn clear_disposed(&mut self) -> Result<(), Error> {
        if self.phase == Phase::Vacant {
            return Ok(());
        }
        if self.phase != Phase::Complete
            || self.disposal != Disposal::Complete
            || self.clips.is_some()
            || self.preview.is_some()
        {
            return Err(Error::Capacity);
        }
        self.handle = 0;
        self.disposal = Disposal::Unstarted;
        self.phase = Phase::Vacant;
        Ok(())
    }
    /// Test-only observation while this actual allocation stays strongly rooted;
    /// never a production identity or lifetime/admission authority.
    #[cfg(test)]
    pub(crate) fn observation(&self) -> Option<(usize, usize, u64, bool)> {
        let owner = self
            .clips
            .as_ref()
            .or_else(|| self.preview.as_ref().map(|p| &p.clips))?;
        Some((
            owner.arc().as_ptr() as usize,
            std::sync::Arc::strong_count(owner.arc()),
            self.handle,
            self.preview.is_some(),
        ))
    }
    #[cfg(test)]
    pub(crate) fn pending(&self) -> bool {
        self.disposal == Disposal::Pending
    }
}
impl PreviewCustody {
    pub(crate) fn visit_audio_storage<'a>(
        &'a self,
        l: &mut crate::AudioRootStorageLedger<'a>,
        external: Option<&'a MidiArrangementPreview>,
    ) -> Result<(), Error> {
        use crate::audio_storage::Owner;
        self.capacity_at(external)?;
        let Self {
            clips,
            preview,
            handle,
            phase: _,
            disposal: _,
            #[cfg(test)]
                fail_after: _,
            #[cfg(test)]
                panic_after: _,
            #[cfg(test)]
                probe: _,
        } = self;
        let owner = clips.as_ref().or_else(|| {
            preview
                .as_ref()
                .or(external)
                .map(|MidiArrangementPreview { clips }| clips)
        });
        if let Some(owner) = owner {
            l.shared(
                owner.arc(),
                owner.parts(),
                Owner::exclusive(owner),
                *handle,
                None,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_empty_preview_provenance_never_upgrades_legacy_origins() {
        let mut custody = PreviewCustody::default();
        let project = hex_project::Project::new(48_000).unwrap();
        assert!(MidiArrangementPreview::default().clips.parts().is_none());
        assert!(
            MidiArrangementPreview::from_sorted_clips(Vec::new())
                .clips
                .parts()
                .is_none()
        );
        if !crate::pcm_arc::native_layout_known() {
            assert_eq!(
                custody.reserve(&project),
                Err(Error::UnsupportedConstructor)
            );
            assert_eq!(custody.capacity().unwrap(), AudioPreviewCapacity::default());
            eprintln!("hex preparation evidence: audio_preview_origin; branch=unsupported-refusal");
            return;
        }
        custody.reserve(&project).unwrap();
        let before = custody.observation().unwrap();
        let declared = custody.capacity().unwrap();
        custody.construct().unwrap();
        let after = custody.observation().unwrap();
        assert_eq!((before.0, before.1, before.2), (after.0, after.1, after.2));
        assert!(!before.3 && after.3);
        assert_eq!(custody.capacity().unwrap(), declared);
        // Test-only raw-origin ingress keeps actual allocation identity, but
        // cannot invent the constructor declaration or erase its obligation.
        let actual = &custody.preview.as_ref().unwrap().clips;
        let raw = OwnedShared::untracked(std::sync::Arc::clone(actual.arc()));
        assert!(raw.parts().is_none());
        assert!(std::sync::Arc::ptr_eq(actual.arc(), raw.arc()));
        drop(raw);
        custody.dispose_next().unwrap();
        custody.dispose_next().unwrap();
        custody.clear_disposed().unwrap();
        assert_eq!(custody.capacity().unwrap(), AudioPreviewCapacity::default());
        eprintln!(
            "hex preparation evidence: audio_preview_origin; supported-controls=completed; raw-default-emptyvec-remain-unknown"
        );
    }
}
