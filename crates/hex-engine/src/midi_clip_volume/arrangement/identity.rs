//! The sealed constructor retains this exact metadata witness alongside its
//! existing allocation declarations. Curve references retain identity but are
//! charged as conservative control metadata, never described as born DATA.
use super::*;
use crate::audio_storage::{Location, Owner};
use crate::{AudioRootStorageLedger, AudioStorageError};

#[derive(Debug)]
struct RowIdentity {
    clip: ClipId,
    start: BeatTime,
    start_frame: u64,
    endpoint: Option<VolumeEndpoint>,
    program: VolumePreparationIdentity,
}
#[derive(Debug)]
pub(crate) struct ArrangementVolumeIdentity {
    rows: Vec<RowIdentity>,
}
impl ArrangementVolumeIdentity {
    pub(super) fn prepare(value: &PreparedArrangementVolumes) -> Result<Self, Invalid> {
        let mut rows = Vec::new();
        rows.try_reserve_exact(value.rows.len())
            .map_err(|_| Invalid("could not reserve clip volume identity rows"))?;
        for row in &value.rows {
            rows.push(RowIdentity {
                clip: row.clip,
                start: row.start,
                start_frame: row.start_frame,
                endpoint: row.endpoint,
                program: row.program.preparation_identity(),
            });
        }
        Ok(Self { rows })
    }
    pub(crate) fn matches(&self, value: &PreparedArrangementVolumes) -> bool {
        self.rows.len() == value.rows.len()
            && self.rows.iter().zip(&value.rows).all(|(old, new)| {
                old.clip == new.clip
                    && old.start == new.start
                    && old.start_frame == new.start_frame
                    && old.endpoint == new.endpoint
                    && old.program.matches(&new.program)
            })
    }
    pub(crate) fn quoted_row_bytes(rows: usize) -> Result<usize, AudioStorageError> {
        rows.checked_mul(std::mem::size_of::<RowIdentity>())
            .ok_or(AudioStorageError::Overflow)
    }
    pub(crate) fn retained_bytes(&self) -> Result<usize, AudioStorageError> {
        self.rows.iter().try_fold(
            Self::quoted_row_bytes(self.rows.capacity())?,
            |bytes, row| {
                bytes
                    .checked_add(row.program.retained_control_metadata_bytes())
                    .ok_or(AudioStorageError::Overflow)
            },
        )
    }
    pub(crate) fn visit_storage<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.vec(&self.rows)?;
        for row in &self.rows {
            ledger.bytes(
                Owner::exclusive(row),
                Location("clip-volume-identity-control-metadata-allowance", 0),
                row.program.retained_control_metadata_bytes(),
            )?;
        }
        Ok(())
    }
}
impl PreparedArrangementVolumes {
    pub(crate) fn visit_storage<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.vec(&self.rows)?;
        for row in &self.rows {
            ledger.bytes(
                Owner::exclusive(row),
                Location("clip-volume-control-metadata-allowance", 0),
                row.program.retained_control_metadata_bytes(),
            )?;
        }
        Ok(())
    }
    /// Preconstruction retained bound: actual rows plus the sealed identity
    /// rows, and a conservative curve backing charge for each retained owner.
    pub(crate) fn closed_quote(track: &Track) -> Result<usize, AudioStorageError> {
        let mut count = 0usize;
        let mut metadata = 0usize;
        for clip in track.midi_clips() {
            if !clip.volume().is_unity() {
                count = count.checked_add(1).ok_or(AudioStorageError::Overflow)?;
                metadata = metadata
                    .checked_add(
                        clip.volume()
                            .retained_control_metadata_bytes()
                            .ok_or(AudioStorageError::Overflow)?,
                    )
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        count
            .checked_mul(std::mem::size_of::<PreparedArrangementClipVolume>())
            .and_then(|n| n.checked_add(ArrangementVolumeIdentity::quoted_row_bytes(count).ok()?))
            .and_then(|n| n.checked_add(metadata.checked_mul(2)?))
            .ok_or(AudioStorageError::Overflow)
    }
}
