//! Finite Arrangement clip-volume owners, prepared once per track. Rendering
//! resolves one row per note span; no callback scan or per-frame beat conversion.
use super::*;
use hex_project::{ClipId, MidiClipVolumePoint, Project, Track};
mod identity;
pub(crate) use identity::ArrangementVolumeIdentity;

#[derive(Debug, Clone)]
pub(crate) struct PreparedArrangementVolumes {
    rows: Vec<PreparedArrangementClipVolume>,
    metadata_bytes: usize,
}
#[derive(Debug, Clone)]
pub(crate) struct PreparedArrangementClipVolume {
    pub(crate) clip: ClipId,
    pub(crate) start: BeatTime,
    pub(crate) start_frame: u64,
    pub(crate) program: PreparedMidiClipVolume,
    pub(crate) endpoint: Option<VolumeEndpoint>,
}
impl PreparedArrangementVolumes {
    pub(crate) fn preparation_identity(&self) -> Result<ArrangementVolumeIdentity, Invalid> {
        ArrangementVolumeIdentity::prepare(self)
    }
    /// `track` must be the actual canonical owner in `project`; timing is its
    /// borrowed effective clock, never a newly prepared/cloned map. Empty curves
    /// are unity and need no rows, backing allocation, or curve metadata lease.
    pub(crate) fn prepare<T: MidiTempoBreaks + ?Sized>(
        project: &Project,
        track: &Track,
        timing: &T,
    ) -> Result<Option<Self>, Invalid> {
        if !project
            .track(track.id())
            .is_some_and(|owner| std::ptr::eq(owner, track))
            || timing.sample_rate() != project.sample_rate()
        {
            return Err(Invalid("clip volume arrangement owner/timing mismatch"));
        }
        let mut count = 0usize;
        let mut points = 0usize;
        let mut metadata_bytes = 0usize;
        for clip in track.midi_clips() {
            if clip.volume().is_unity() {
                continue;
            }
            count = count
                .checked_add(1)
                .filter(|n| *n <= hex_project::MAX_CLIPS)
                .ok_or(Invalid("arrangement clip volume row limit reached"))?;
            points = points
                .checked_add(clip.volume().points().len())
                .filter(|n| *n <= hex_project::MAX_MIDI_VOLUME_POINTS)
                .ok_or(Invalid("arrangement clip volume point limit reached"))?;
            metadata_bytes = metadata_bytes
                .checked_add(
                    clip.volume()
                        .retained_control_metadata_bytes()
                        .ok_or(Invalid("arrangement clip volume metadata size overflow"))?,
                )
                .ok_or(Invalid("arrangement clip volume metadata size overflow"))?;
        }
        if count == 0 {
            return Ok(None);
        }
        check_budget(count, metadata_bytes)?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve arrangement clip volume rows"))?;
        check_budget(rows.capacity(), metadata_bytes)?;
        for clip in track.midi_clips() {
            if clip.volume().is_unity() {
                continue;
            }
            let program = PreparedMidiClipVolume::prepare(
                clip.volume(),
                clip.source_playback().map_err(|error| Invalid(error.0))?,
                timing,
            )?;
            let endpoint = program.finite_endpoint(
                timing,
                VolumePlacement::Arrangement {
                    start: clip.start(),
                },
            )?;
            let start_frame = timing
                .beat_to_frame(clip.start())
                .map_err(|_| Invalid("arrangement clip volume start mapping is unavailable"))?;
            if endpoint.is_some_and(|end| end.frame < start_frame) {
                return Err(Invalid("arrangement clip volume end precedes start"));
            }
            rows.push(PreparedArrangementClipVolume {
                clip: clip.id(),
                start: clip.start(),
                start_frame,
                program,
                endpoint,
            });
        }
        rows.sort_unstable_by_key(|row| row.clip);
        if rows.windows(2).any(|pair| pair[0].clip == pair[1].clip) {
            return Err(Invalid("duplicate arrangement clip volume identity"));
        }
        Ok(Some(Self {
            rows,
            metadata_bytes,
        }))
    }
    pub(crate) fn clip(&self, clip: ClipId) -> Option<&PreparedArrangementClipVolume> {
        self.rows
            .binary_search_by_key(&clip, |row| row.clip)
            .ok()
            .and_then(|index| self.rows.get(index))
    }
    /// Actual Vec backing for the existing root ledger's inspection. Borrowing
    /// it conveys no declaration, allocation identity, or closed-storage proof.
    #[cfg(test)]
    pub(crate) fn rows(&self) -> &Vec<PreparedArrangementClipVolume> {
        &self.rows
    }
    pub(crate) fn allocated_row_bytes(&self) -> Result<usize, Invalid> {
        self.rows
            .capacity()
            .checked_mul(std::mem::size_of::<PreparedArrangementClipVolume>())
            .ok_or(Invalid("arrangement clip volume row size overflow"))
    }
    /// Conservative curve Arc header/payload count, charged once per row even
    /// if canonical owners share a curve. Inline/Vec rows are accounted separately.
    pub(crate) fn retained_control_metadata_bytes(&self) -> usize {
        self.metadata_bytes
    }
}
impl PreparedArrangementClipVolume {
    /// Use the same prepared clock as this row. Outgoing tails retain this
    /// owner's evaluated endpoint scalar, including through later handoffs.
    pub(crate) fn segment_or_hold<T: MidiTempoBreaks + ?Sized>(
        &self,
        timing: &T,
        frame: u64,
        frames: usize,
    ) -> Result<VolumeRamp, Invalid> {
        if frames == 0 || frames > VOLUME_QUERY_FRAMES || frame.checked_add(frames as u64).is_none()
        {
            return Err(Invalid(
                "arrangement clip volume query exceeds frame capacity",
            ));
        }
        if frame < self.start_frame {
            return Err(Invalid("arrangement clip volume query precedes clip"));
        }
        let endpoint = self
            .endpoint
            .ok_or(Invalid("clip volume source window is silent"))?;
        if frame >= endpoint.frame {
            return Ok(VolumeRamp {
                start_frame: endpoint.frame,
                end_frame: u64::MAX,
                start_gain: endpoint.gain,
                end_gain: endpoint.gain,
                next_boundary: None,
            });
        }
        self.program
            .segment_at(
                timing,
                VolumePlacement::Arrangement { start: self.start },
                frame,
                frames,
            )?
            .ok_or(Invalid(
                "arrangement clip volume active segment is unavailable",
            ))
    }
}
fn check_budget(rows: usize, metadata: usize) -> Result<(), Invalid> {
    // The model's global row/point limits bound this control owner. Metadata
    // includes conservative Arc headers/alignment, not just authored payload.
    let overhead = 2usize
        .checked_mul(std::mem::size_of::<usize>())
        .and_then(|n| {
            n.checked_add(
                std::mem::align_of::<usize>().max(std::mem::align_of::<MidiClipVolumePoint>()) - 1,
            )
        })
        .ok_or(Invalid("arrangement clip volume metadata budget overflow"))?;
    let max_metadata = hex_project::MAX_MIDI_VOLUME_POINTS
        .checked_mul(std::mem::size_of::<MidiClipVolumePoint>())
        .and_then(|n| n.checked_add(hex_project::MAX_CLIPS.checked_mul(overhead)?))
        .ok_or(Invalid("arrangement clip volume metadata budget overflow"))?;
    let row_bytes = rows
        .checked_mul(std::mem::size_of::<PreparedArrangementClipVolume>())
        .ok_or(Invalid("arrangement clip volume row size overflow"))?;
    row_bytes
        .checked_add(metadata)
        .ok_or(Invalid("arrangement clip volume total size overflow"))?;
    if rows > hex_project::MAX_CLIPS || metadata > max_metadata {
        return Err(Invalid(
            "arrangement clip volume exceeds bounded metadata budget",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod render_tests;
#[cfg(test)]
mod tests;
