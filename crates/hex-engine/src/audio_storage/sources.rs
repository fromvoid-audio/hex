use super::*;
use crate::audio_only_inputs::{OwnedAudioSource, SourceKind};
impl<'a> AudioRootStorageLedger<'a> {
    /// Actual PCM capacity only. This does not upgrade a legacy Audio handle to
    /// constructor-paired Arc provenance or attest its reclamation. Native banks
    /// use the identical backing/location declaration, so aliases share one row.
    pub(crate) fn resident_capacity(
        &mut self,
        audio: &'a crate::Audio,
    ) -> Result<(), AudioStorageError> {
        let capacity = audio.frames.values.capacity();
        self.data(
            Owner::shared(&audio.frames),
            Location("resident-buffer", 0),
            Declaration {
                bytes: capacity
                    .checked_mul(std::mem::size_of::<crate::Frame>())
                    .ok_or(AudioStorageError::Overflow)?,
                capacity,
                element_bytes: std::mem::size_of::<crate::Frame>(),
                overhead: 0,
            },
            None,
        )
    }
    pub(crate) fn resident_capacity_frames(&self) -> Result<u64, AudioStorageError> {
        self.summary()?;
        self.data
            .iter()
            .filter(|row| row.location == Location("resident-buffer", 0))
            .try_fold(0_u64, |frames, row| {
                frames
                    .checked_add(
                        u64::try_from(row.declaration.capacity)
                            .map_err(|_| AudioStorageError::Overflow)?,
                    )
                    .ok_or(AudioStorageError::Overflow)
            })
    }
    pub(crate) fn inputs(&mut self, inputs: &'a OwnedAudioInputs) -> Result<(), AudioStorageError> {
        self.vec(&inputs.sources)?;
        self.vec(&inputs.aliases)?;
        for (_, source) in &inputs.sources {
            self.source(source)?;
        }
        for alias in &inputs.aliases {
            if let Some(owner) = &alias.owner {
                let original = &inputs
                    .sources
                    .get(alias.source)
                    .ok_or(AudioStorageError::SourceMismatch)?
                    .1;
                if !crate::audio_input_storage::same(original, owner)
                    || crate::audio_input_storage::parts(original)?
                        != crate::audio_input_storage::parts(owner)?
                {
                    return Err(AudioStorageError::SourceMismatch);
                }
                self.source(owner)?;
            }
        }
        // Scalar caller byte declarations cannot prove disjointness with these
        // actual owners. Preserve that uncertainty rather than subtract a sum.
        if inputs.reservation.caller_retained_bytes != 0
            || inputs.reservation.caller_construction_bytes != 0
        {
            self.unknown()?;
        }
        Ok(())
    }
    pub(crate) fn source(&mut self, source: &'a OwnedAudioSource) -> Result<(), AudioStorageError> {
        match &source.kind {
            SourceKind::Resident(audio) => {
                self.resident(audio, Owner::exclusive(source), source.handle)
            }
            SourceKind::Streamed(owner) => {
                owner.inspect_scalar_owned_storage();
                self.shared(
                    owner.arc(),
                    owner.parts(),
                    Owner::exclusive(source),
                    source.handle,
                    Some(owner.as_ref()),
                )
            }
        }
    }
    pub(crate) fn resident(
        &mut self,
        audio: &'a crate::Audio,
        field: Owner<'a>,
        handle: u64,
    ) -> Result<(), AudioStorageError> {
        let allocations = audio
            .pcm_arc_receipt
            .and_then(|r| r.checked_allocations(audio))
            .ok_or(AudioStorageError::UnknownStorage)?;
        self.shared(&audio.frames, Some(allocations[0]), field, handle, None)?;
        let (payload, overhead) = allocations[1];
        self.data(
            Owner::shared(&audio.frames),
            Location("resident-buffer", 0),
            Declaration {
                bytes: payload
                    .checked_add(overhead)
                    .ok_or(AudioStorageError::Overflow)?,
                capacity: audio.frames.values.capacity(),
                element_bytes: std::mem::size_of::<crate::Frame>(),
                overhead,
            },
            None,
        )
    }
}
