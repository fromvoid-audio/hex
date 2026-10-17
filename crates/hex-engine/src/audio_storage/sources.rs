use super::*;
use crate::audio_only_inputs::{SourceStorage, SourceStorageKind};
impl<'a> AudioRootStorageLedger<'a> {
    /// Visit an actual closed source field retained by a media/control owner.
    /// This leaf inventory is not a declaration for its enclosing container.
    pub fn borrow_native_source_on_control(
        &mut self,
        source: &'a crate::OwnedAudioSource,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            crate::audio_input_storage::parts(source)?;
            ledger.root(source, role)?;
            ledger.source(source)
        })
    }
    /// Actual native original or freshly prepared stereo PCM joins the SAME
    /// union. Old/raw derived origin remains unknown; IDs/counts cannot mint it.
    pub fn borrow_native_audio_on_control(
        &mut self,
        audio: &'a crate::Audio,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            let handle = audio
                .native_handle
                .map(|h| h.get())
                .ok_or(AudioStorageError::UnknownStorage)?;
            ledger.root(audio, role)?;
            ledger.resident(audio, Owner::exclusive(audio), handle)
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
    pub(crate) fn source(
        &mut self,
        source: &'a impl SourceStorage,
    ) -> Result<(), AudioStorageError> {
        match source.storage_kind() {
            SourceStorageKind::Resident(audio) => {
                audio
                    .pcm_arc_receipt
                    .and_then(|receipt| receipt.checked_allocations(audio))
                    .ok_or(AudioStorageError::UnknownStorage)?;
                self.resident(audio, Owner::exclusive(source), source.storage_handle())
            }
            SourceStorageKind::ResidentMono(owner) => {
                self.shared(
                    owner.arc(),
                    owner.parts(),
                    Owner::exclusive(source),
                    source.storage_handle(),
                    None,
                )?;
                let crate::audio_only_inputs::MonoSourceData {
                    sample_rate: _,
                    samples,
                } = owner.as_ref();
                self.vec(samples)
            }
            SourceStorageKind::Streamed(owner) => {
                owner.inspect_scalar_owned_storage();
                self.shared(
                    owner.arc(),
                    owner.parts(),
                    Owner::exclusive(source),
                    source.storage_handle(),
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
            .and_then(|r| r.checked_native_allocations(audio))
            .ok_or(AudioStorageError::UnknownStorage)?;
        if audio.native_handle.map(|h| h.get()) != Some(handle) {
            return Err(AudioStorageError::SourceMismatch);
        }
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
