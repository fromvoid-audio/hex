//! Persistent nonempty compiler preview output; no raw preview adoption.
use super::*;
#[derive(Clone, Copy)]
pub(crate) struct CompilerPreviewCost {
    pub(crate) retained: usize,
    pub(crate) peak: usize,
    pub(crate) clips: usize,
}
impl PreviewCustody {
    pub(crate) fn compiler_cost(
        project: &hex_project::Project,
    ) -> Result<CompilerPreviewCost, Error> {
        let clips = project.tracks().iter().try_fold(0usize, |sum, track| {
            sum.checked_add(track.midi_clips().len())
                .filter(|count| *count <= hex_project::MAX_CLIPS)
                .ok_or(Error::Overflow)
        })?;
        let (body, header) = OwnedShared::<[MidiClipPreview]>::declared_slice_parts(clips)
            .ok_or(Error::UnsupportedConstructor)?;
        let samples = clips
            .checked_mul(super::super::MAX_MIDI_PREVIEW_SAMPLES_PER_CLIP)
            .and_then(|n| n.checked_mul(std::mem::size_of::<super::super::MidiPreviewSample>()))
            .ok_or(Error::Overflow)?;
        let retained = body
            .checked_add(header)
            .and_then(|n| n.checked_add(samples))
            .ok_or(Error::Overflow)?;
        // Vec->shared slice temporarily retains BOTH real row backings. The
        // sequential ordinal selection also coexists with the current samples.
        let peak = retained
            .checked_add(body)
            .and_then(|n| {
                n.checked_add(
                    super::super::MAX_MIDI_PREVIEW_SAMPLES_PER_CLIP * std::mem::size_of::<usize>(),
                )
            })
            .ok_or(Error::Overflow)?;
        Ok(CompilerPreviewCost {
            retained,
            peak,
            clips,
        })
    }
    pub(crate) fn reserve_compiler(&mut self, project: &hex_project::Project) -> Result<(), Error> {
        if self.phase != Phase::Vacant
            || self.clips.is_some()
            || self.preview.is_some()
            || self.handle != 0
            || self.draft.capacity() != 0
            || self.disposal != Disposal::Unstarted
        {
            return Err(Error::Capacity);
        }
        let cost = Self::compiler_cost(project)?;
        self.handle = crate::audio_only_inputs::handle()?;
        self.phase = Phase::Reserved;
        self.draft
            .try_reserve_exact(cost.clips)
            .map_err(|_| Error::Allocation)?;
        if self.draft.capacity() != cost.clips {
            return Err(Error::ReservationExceeded);
        }
        Ok(())
    }
    pub(crate) fn compiler_destination(&mut self) -> Result<&mut Vec<MidiClipPreview>, Error> {
        if self.phase != Phase::Reserved
            || self.handle == 0
            || self.clips.is_some()
            || self.preview.is_some()
            || !self.draft.is_empty()
        {
            return Err(Error::SourceMismatch);
        }
        Ok(&mut self.draft)
    }
    pub(crate) fn finish_compiler(&mut self) -> Result<(), Error> {
        if self.phase != Phase::Reserved
            || self.handle == 0
            || self.clips.is_some()
            || self.preview.is_some()
        {
            return Err(Error::SourceMismatch);
        }
        self.draft.sort_unstable_by_key(|clip| clip.clip_id);
        if self
            .draft
            .windows(2)
            .any(|pair| pair[0].clip_id >= pair[1].clip_id)
        {
            return Err(Error::SourceMismatch);
        }
        // Closed metadata consists of Copy scalars and Vec<Copy sample>; its
        // constructor has no provider/user code or recoverable failure after
        // take. Allocation failure follows the existing fatal-OOM convention.
        let clips = if self.draft.is_empty() {
            drop(std::mem::take(&mut self.draft));
            OwnedShared::new_owned_empty_slice()
        } else {
            OwnedShared::from_owned_vec(std::mem::take(&mut self.draft))
        };
        self.preview = Some(MidiArrangementPreview { clips });
        self.phase = Phase::Installed;
        self.capacity()?;
        Ok(())
    }
}
