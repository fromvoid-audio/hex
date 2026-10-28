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
    voice_rows: Vec<PreparedArrangementVoiceRow>,
    voice_programs: Vec<super::super::voice::PreparedVoiceGainIdentity>,
    metadata_bytes: usize,
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
        let mut voice_rows = Vec::new();
        voice_rows
            .try_reserve_exact(value.voice_rows.len())
            .map_err(|_| Invalid("could not reserve voice gain identity rows"))?;
        if voice_rows.capacity() > value.voice_rows.len() {
            return Err(Invalid("voice gain identity rows exceed their quote"));
        }
        voice_rows.extend_from_slice(&value.voice_rows);
        let mut voice_programs = Vec::new();
        voice_programs
            .try_reserve_exact(value.voice_programs.len())
            .map_err(|_| Invalid("could not reserve voice gain program identities"))?;
        if voice_programs.capacity() > value.voice_programs.len() {
            return Err(Invalid("voice gain program identities exceed their quote"));
        }
        for program in &value.voice_programs {
            voice_programs.push(program.preparation_identity()?);
        }
        Ok(Self {
            rows,
            voice_rows,
            voice_programs,
            metadata_bytes: value.metadata_bytes,
        })
    }
    pub(crate) fn matches(&self, value: &PreparedArrangementVolumes) -> bool {
        self.metadata_bytes == value.metadata_bytes
            && self.voice_rows == value.voice_rows
            && self.voice_programs.len() == value.voice_programs.len()
            && self
                .voice_programs
                .iter()
                .zip(&value.voice_programs)
                .all(|(old, new)| old.matches(new))
            && self.rows.len() == value.rows.len()
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
        let bytes = self.rows.iter().try_fold(
            Self::quoted_row_bytes(self.rows.capacity())?,
            |bytes, row| {
                bytes
                    .checked_add(row.program.retained_control_metadata_bytes())
                    .ok_or(AudioStorageError::Overflow)
            },
        )?;
        let bytes =
            bytes
                .checked_add(
                    self.voice_rows
                        .capacity()
                        .checked_mul(std::mem::size_of::<PreparedArrangementVoiceRow>())
                        .ok_or(AudioStorageError::Overflow)?,
                )
                .and_then(|n| {
                    n.checked_add(self.voice_programs.capacity().checked_mul(
                        std::mem::size_of::<super::super::voice::PreparedVoiceGainIdentity>(),
                    )?)
                })
                .ok_or(AudioStorageError::Overflow)?;
        self.voice_programs
            .iter()
            .try_fold(bytes, |bytes, program| {
                program
                    .retained_bytes()
                    .and_then(|size| bytes.checked_add(size))
                    .ok_or(AudioStorageError::Overflow)
            })
    }
    pub(crate) fn visit_storage<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.vec(&self.rows)?;
        if !self.voice_rows.is_empty() {
            ledger.vec(&self.voice_rows)?;
            ledger.vec(&self.voice_programs)?;
            for program in &self.voice_programs {
                program.visit_storage(ledger)?;
            }
        }
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
        if self.has_voice_gains() {
            ledger.vec(&self.voice_rows)?;
            ledger.vec(&self.voice_programs)?;
            for program in &self.voice_programs {
                program.visit_storage(ledger)?;
            }
        }
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
    pub(crate) fn closed_quote(
        track: &Track,
        voice: &VoicePreparationQuote,
    ) -> Result<usize, AudioStorageError> {
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
            .and_then(|n| n.checked_add(voice.retained))
            .ok_or(AudioStorageError::Overflow)
    }
}
