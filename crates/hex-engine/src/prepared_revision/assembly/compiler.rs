//! Actual compiler descriptor outputs in the existing revision custodian.
//! These local costs neither admit a caller ledger nor authorize publication.
use super::*;

fn add(a: usize, b: usize) -> Result<usize, Error> {
    a.checked_add(b).ok_or(Error::Overflow)
}
fn bytes<T>(count: usize) -> Result<usize, Error> {
    std::alloc::Layout::array::<T>(count).map_err(|_| Error::Overflow)?;
    count
        .checked_mul(std::mem::size_of::<T>())
        .ok_or(Error::Overflow)
}
impl RevisionAssembly {
    pub(crate) fn compiler_descriptor_allowance(
        project: &hex_project::Project,
    ) -> Result<usize, Error> {
        if project.tracks().len() > MAX_TRACKS {
            return Err(Error::UnsupportedGraph);
        }
        // Every track is a conservative possible descriptor; actual selection
        // happens against the same compiled arrangement. No PCM is multiplied.
        let mut result = bytes::<PreparedMutableTrackRevision>(project.tracks().len())?;
        for track in project.tracks() {
            let source = project
                .track(project.midi_source_track(track.id()))
                .ok_or(Error::SourceMismatch)?;
            if track.devices().len() > MAX_DEVICES_PER_TRACK
                || source.midi_clips().len() > MAX_CLIPS
            {
                return Err(Error::UnsupportedGraph);
            }
            result = add(
                result,
                bytes::<PreparedDeviceIdentity>(track.devices().len())?,
            )?;
            result = add(
                result,
                bytes::<PreparedArrangementMidiSource>(source.midi_clips().len())?,
            )?;
        }
        Ok(result)
    }
    pub(crate) fn compiler_construction_bytes(
        project: &hex_project::Project,
    ) -> Result<usize, Error> {
        let mut total = Self::compiler_descriptor_allowance(project)?;
        for parts in [
            OwnedShared::<PlaybackProject>::declared_parts(),
            OwnedShared::<PreparedSessionInventory>::declared_parts(),
            OwnedShared::<MidiArrangementPreview>::declared_parts(),
            OwnedShared::<PreparedRevision>::declared_parts(),
        ] {
            let (body, header) = parts.ok_or(Error::UnsupportedConstructor)?;
            total = add(total, add(body, header)?)?;
        }
        add(
            total,
            super::super::compiled_reader::ReaderState::declared_bytes()?,
        )
    }
    pub(crate) fn reserve_compiled(
        &mut self,
        project: &hex_project::Project,
        arrangement: &PlaybackProject,
    ) -> Result<(), Error> {
        self.require_vacant()?;
        let allowance = Self::compiler_descriptor_allowance(project)?;
        self.reserve_roots()?;
        super::super::prepare_mutable_track_revisions_into(
            project,
            arrangement,
            &mut self.mutable_descriptors,
            allowance,
        )
        .map_err(|_| Error::SourceMismatch)?;
        self.capacity()?;
        Ok(())
    }
}
